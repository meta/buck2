/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// TraceEvent spec used in this file documented here:
// https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU/preview?tab=t.0
// Note: "rendering" centric stuff like cname colors are not supported:
// https://github.com/google/perfetto/issues/208, we'd have to switch to the
// protobuf API
//
// Originally [perfetto](https://perfetto.dev/docs/) was built by google for
// chrome tracing, and then later for android,  linux system tracing, etc. It
// has a concept of "processes" and "threads", which for "normal" traces are
// directly translated into TraceEvents json objects. Field like `pid` and `tid`
// are traditionally process id and thread id.
//
// In these traces, it's not really practical to assign the actual  pid/tid that
// produced the BuckEvents in the logs to the TraceEvent objects. MUCH of buck's
// work is done in asynchronous futures, and the thread assignments for them are
// (somewhat) irrelevant. If an action execution future gets moved between
// executor threads, say, we would still like to represent the spans as relating
// to each other/owning each other.
//
// The json TraceEvent api is interpreted by the perfetto viewer to assume
// relationships between events that it uses to render them. The "thread id"
// parameter is used to group duration events together in the same horizontal
// "track", because a thread in a program is normally executing synchronous code
// (partcularly at the time perfetto was first written).
//
// We exploit that, and treat the `tid` parameter as not a literal thread id,
// but as a "track id", assuming that perfetto will render any events with the
// same tid in the same horizontal track.
//
// When processing the async spans in the logs, we keep track of which displayed
// spans are "open" over time and which ones are assigned to tracks. As tracks
// fill up, we just stop displaying new spans in any tracks, as perfetto doesn't
// have a concept of duration events spanning across each other, they may only
// nest.
//
//   Note: the TraceEvent data model does allow for async events, which we may
//   want to experiment with. whether tracery supports them may determine if we
//   can use them.
//
// Generally this track assignment works really well for loads, because all
// loads occur on a limited pool of local executors, so we can generally have
// perfetto render enough tracks/threads to render all concurrently running
// loads. This falls apart for action execution, because we allow many thousands
// of futures to be created and be running in parallel. Perfetto's UI tops out
// at ~200-256 tracks/threads, so we will probably NEVER be able to render them
// all.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::path_arg::PathArg;
use buck2_common::convert::ProstDurationExt;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_event_log::read::EventLogPathBuf;
use buck2_event_log::stream_value::StreamValue;
use buck2_event_log::utils::Invocation;
use buck2_event_observer::display;
use buck2_event_observer::display::CriticalPathEntryDisplay;
use buck2_event_observer::display::TargetDisplayOptions;
use buck2_event_observer::unpack_event::UnpackedBuckEvent;
use buck2_event_observer::unpack_event::unpack_event;
use buck2_events::BuckEvent;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use derive_more::Display;
use dupe::Dupe;
use flate2::Compression;
use flate2::write::GzEncoder;
use futures::TryStreamExt;
use futures::stream::BoxStream;
use serde::Serialize;
use serde_json::json;

/// Generates a Chrome trace from a buck2 event log.
#[derive(Debug, clap::Parser)]
pub struct ChromeTraceCommand {
    #[clap(flatten)]
    pub output: OutputArgs,

    /// The path to read the event log from.
    #[clap(
        long = "path",
        help = "A path to an event-log file to read from. Only works for log files with a single command in them. If no event-log is passed, the most recent one will be used.",
        value_name = "PATH",
        // Hide because `event_log` below subsumes this.
        hide = true
    )]
    pub event_log_path: Option<PathArg>,
    #[clap(
        long,
        help = "Places a global instant event at the specified time. Floating point seconds and integer nanoseconds unixtime is accepted.",
        value_name = "TIMESTAMP DESCRIPTION",
        number_of_values = 2
    )]
    instant: Vec<String>,

    #[clap(
        long,
        help = "How many tracks to render events into",
        value_name = "MAX_TRACKS"
    )]
    pub max_tracks: Option<u64>,

    #[clap(flatten)]
    pub(crate) event_log: EventLogOptions,
}

const TRACE_PATH_HELP: &str = "Where to write the chrome trace JSON. If a directory is passed, the filename of the event log will be used as a base filename.";

#[derive(Debug, clap::Parser)]
#[group(required = true, multiple = true)]
pub struct OutputArgs {
    #[cfg(fbcode_build)]
    #[clap(long, help = TRACE_PATH_HELP)]
    pub trace_path: Option<PathArg>,
    #[cfg(not(fbcode_build))]
    #[clap(long, help = TRACE_PATH_HELP)]
    pub trace_path: PathArg,

    /// Uploads the result to manifold and generates a perfetto link for you
    #[cfg(fbcode_build)]
    #[clap(long)]
    pub upload: bool,
}

struct ChromeTraceFirstPass {
    /// Track assignment needs to know, when it sees a SpanStart, whether that
    /// span is going to be included in the final trace.
    /// But some spans need to be filtered based on later events, like:
    ///
    /// 1. We shouldn't assign tracks to StartLoad events whose SpanEnd records
    ///    a really short duration.
    ///
    /// 2. We shouldn't assign tracks to ActionExecutionStart events who have
    ///    no child LocalStage spans.
    ///
    /// 3. (eventually) We should assign tracks to ActionExecutionStart events
    ///    only if they appear in the CriticalPath, but the CriticalPath is one
    ///    of the last events.
    ///
    /// So this first pass builds up several lists of "interesting" span IDs.
    pub long_analyses: BuckMutSet<buck2_events::span::SpanId>,
    pub long_loads: BuckMutSet<buck2_events::span::SpanId>,
    pub long_load_packages: BuckMutSet<buck2_events::span::SpanId>,
    pub local_actions: BuckMutSet<buck2_events::span::SpanId>,
    pub critical_path_action_keys: BuckMutSet<buck2_data::ActionKey>,
    pub critical_path_span_ids: BuckMutSet<u64>,
    pub command_start: SystemTime,
    pub command_options: Option<buck2_data::CommandOptions>,
}

impl ChromeTraceFirstPass {
    const LONG_ANALYSIS_CUTOFF: Duration = Duration::from_millis(50);
    const LONG_LOAD_CUTOFF: Duration = Duration::from_millis(50);
    const LONG_LOAD_PACKAGE_CUTOFF: Duration = Duration::from_millis(50);
    fn new() -> Self {
        Self {
            long_analyses: BuckMutSet::default(),
            long_loads: BuckMutSet::default(),
            long_load_packages: BuckMutSet::default(),
            local_actions: BuckMutSet::default(),
            critical_path_action_keys: BuckMutSet::default(),
            critical_path_span_ids: BuckMutSet::default(),
            command_start: SystemTime::UNIX_EPOCH,
            command_options: None,
        }
    }

    fn handle_event(&mut self, event: &BuckEvent) -> buck2_error::Result<()> {
        match event.data() {
            buck2_data::buck_event::Data::SpanStart(start) => {
                if let Some(buck2_data::span_start_event::Data::Command(..)) = start.data.as_ref() {
                    self.command_start = event.timestamp();
                } else if let Some(buck2_data::span_start_event::Data::ExecutorStage(exec)) =
                    start.data.as_ref()
                {
                    // A local stage means that we want to show the entire action execution.
                    use buck2_data::executor_stage_start::Stage;

                    if let Some(Stage::Local(local)) = &exec.stage {
                        use buck2_data::local_stage::Stage;

                        let local_execution = match local.stage.as_ref() {
                            Some(Stage::Queued(..)) => false,
                            Some(Stage::Execute(..)) => true,
                            Some(Stage::MaterializeInputs(..)) => false,
                            Some(Stage::PrepareOutputs(..)) => false,
                            Some(Stage::FinalizeOutputs(..)) => false,
                            Some(Stage::AcquireLocalResource(..)) => false,
                            Some(Stage::WorkerInit(..)) => false,
                            Some(Stage::WorkerExecute(..)) => true,
                            Some(Stage::WorkerQueued(..)) => false,
                            Some(Stage::WorkerWait(..)) => false,
                            None => false,
                        };

                        if local_execution {
                            self.local_actions.insert(event.parent_id().unwrap());
                        }
                    }
                }
            }
            buck2_data::buck_event::Data::SpanEnd(end) => {
                if let Some(buck2_data::span_end_event::Data::Analysis(_)) = end.data.as_ref() {
                    if end
                        .duration
                        .as_ref()
                        .expect("Analysis SpanEnd missing duration")
                        .try_into_duration()?
                        > Self::LONG_ANALYSIS_CUTOFF
                    {
                        self.long_analyses.insert(event.span_id().unwrap());
                    }
                } else if let Some(buck2_data::span_end_event::Data::Load(_)) = end.data.as_ref() {
                    if end
                        .duration
                        .as_ref()
                        .expect("Load SpanEnd missing duration")
                        .try_into_duration()?
                        > Self::LONG_LOAD_CUTOFF
                    {
                        self.long_loads.insert(event.span_id().unwrap());
                    }
                } else if let Some(buck2_data::span_end_event::Data::LoadPackage(_)) =
                    end.data.as_ref()
                {
                    if end
                        .duration
                        .as_ref()
                        .expect("LoadPackage SpanEnd missing duration")
                        .try_into_duration()?
                        > Self::LONG_LOAD_PACKAGE_CUTOFF
                    {
                        self.long_load_packages.insert(event.span_id().unwrap());
                    }
                }
            }
            buck2_data::buck_event::Data::Instant(instant) => {
                if let Some(buck2_data::instant_event::Data::BuildGraphInfo(info)) =
                    instant.data.as_ref()
                {
                    self.critical_path_span_ids = info
                        .critical_path2
                        .iter()
                        .flat_map(|entry| entry.span_ids.iter().copied())
                        .collect()
                } else if let Some(buck2_data::instant_event::Data::CommandOptions(options)) =
                    instant.data.as_ref()
                {
                    self.command_options = Some(*options);
                }
            }
            buck2_data::buck_event::Data::Record(_) => {}
        };
        Ok(())
    }
}

enum SpanTrackAssignment {
    Owned(TrackId),
    Inherited(TrackId),
}

impl SpanTrackAssignment {
    fn get_track_id(&self) -> TrackId {
        match self {
            Self::Owned(tid) => *tid,
            Self::Inherited(tid) => *tid,
        }
    }
}

#[allow(dead_code)] // Process and Thread aren't used at this time, but are included for completeness.
enum ChromeTraceInstantScope {
    Global,
    Process(u64),
    Thread(u64, SpanTrackAssignment),
}
struct ChromeTraceInstant {
    name: String,
    timestamp: SystemTime,
    scope: ChromeTraceInstantScope,
    args: Option<serde_json::Value>,
}

impl ChromeTraceInstant {
    fn into_json(self) -> buck2_error::Result<serde_json::Value> {
        let mut js = json!(
            {
                "name": self.name,
                "ts": self.timestamp.duration_since(SystemTime::UNIX_EPOCH)?.as_micros() as u64,
                "ph": "i", // Chrome trace "instant event"
                "s": match self.scope {
                    ChromeTraceInstantScope::Global => "g",
                    ChromeTraceInstantScope::Process(_) => "p",
                    ChromeTraceInstantScope::Thread(_, _) => "t",
                },
            }
        );
        let obj = js
            .as_object_mut()
            .ok_or(buck2_error::internal_error!("expected a mutable object"))?;

        if let Some(args) = self.args {
            obj.insert("args".to_owned(), json!(args));
        }

        match self.scope {
            ChromeTraceInstantScope::Global => {}
            ChromeTraceInstantScope::Process(process_id) => {
                obj.insert("pid".to_owned(), json!(process_id));
            }
            ChromeTraceInstantScope::Thread(process_id, track) => {
                obj.insert("pid".to_owned(), json!(process_id));
                obj.insert("tid".to_owned(), json!(track.get_track_id().as_u64()?));
            }
        }

        Ok(js)
    }
}

// N.B. "Process" and "Thread" here are chrome/perfetto TraceEvent json object
// terms. See comments at the top of this file about how the pid/tid field map
// to how we use them to represent buck's activity in a trace.
#[allow(dead_code)] // Process isn't used at this time, but is included for completeness.
enum ChromeTraceMetadataKind {
    Process { pid: u64 },
    Thread { pid: u64, tid: u64 },
}

struct ChromeTraceMetadata {
    name: String,
    kind: ChromeTraceMetadataKind,
    labels: Option<Vec<String>>,
    sort_index: Option<u64>,
}

impl ChromeTraceMetadata {
    fn into_json(self) -> buck2_error::Result<serde_json::Value> {
        let (ev_name, pid, tid) = match self.kind {
            ChromeTraceMetadataKind::Process { pid } => ("process_name", pid, None),
            ChromeTraceMetadataKind::Thread { pid, tid } => ("thread_name", pid, Some(tid)),
        };
        let mut js = json!({
            "name": ev_name,
            "ph": "M", // Chrome trace "metadata event"
            "pid": pid,
        });
        let obj = js
            .as_object_mut()
            .ok_or(buck2_error::internal_error!("expected a mutable object"))?;

        if let Some(tid) = tid {
            obj.insert("tid".to_owned(), json!(tid));
        }

        let mut args = serde_json::Map::<String, serde_json::Value>::new();

        args.insert("name".to_owned(), json!(self.name));
        if let Some(labels) = self.labels
            && !labels.is_empty()
        {
            args.insert("labels".to_owned(), json!(labels));
        }
        if let Some(sort_index) = self.sort_index {
            args.insert("sort_index".to_owned(), json!(sort_index));
        }

        obj.insert("args".to_owned(), args.into());

        Ok(js)
    }
}

struct ChromeTraceOpenSpan {
    name: String,
    start: SystemTime,
    process_id: u64,
    track: SpanTrackAssignment,
    categories: Vec<&'static str>,
    // Any misc. per-event unstructured data.
    args: serde_json::Value,
}

struct ChromeTraceClosedSpan {
    open: ChromeTraceOpenSpan,
    duration: Duration,
}

impl ChromeTraceClosedSpan {
    fn into_json(self) -> buck2_error::Result<serde_json::Value> {
        Ok(json!(
            {
                "name": self.open.name,
                "ts": self.open.start.duration_since(SystemTime::UNIX_EPOCH)?.as_micros() as u64,
                "dur": self.duration.as_micros() as u64,
                "ph": "X", // Chrome trace "complete event"
                "pid": self.open.process_id,
                "tid": self.open.track.get_track_id().as_u64()?,
                "cat": self.open.categories.join(","),
                "args": self.open.args,
            }
        ))
    }
}

/// Spans are directed to a category, like "critical-path" or "misc". Spans in a
/// category that would overlap are put on different tracks within that category.
#[derive(Clone, Copy, Debug, Dupe)]
struct TrackId {
    track_key: SpanCategorization,
    track: u64,
}

impl From<TrackId> for String {
    fn from(tid: TrackId) -> String {
        // Outputs like "misc-00", "misc-01", ...
        format!("{}-{:02}", tid.track_key, tid.track)
    }
}

impl TrackId {
    fn as_u64(&self) -> buck2_error::Result<u64> {
        // The choice of constant here is mostly arbitratry. It needs to be
        // larger than the total number of tracks being displayed for any given
        // track_key, which in turn is limited by the max tracks we allow to be
        // allocated, which is limited by perfetto's max track/thread display,
        // which is in the ~200-256 range.
        const TRACK_KEY_MULTIPLIER: u64 = 1000;
        if self.track >= TRACK_KEY_MULTIPLIER {
            return Err(buck2_error::internal_error!(
                "Track id {:?} has a track component {} that exceeds the key multiplier {}, increase the key multiplier",
                self,
                self.track,
                TRACK_KEY_MULTIPLIER
            ));
        }
        Ok((self.track_key as u64) * TRACK_KEY_MULTIPLIER + self.track)
    }
}

struct TrackIdAllocator {
    unused_track_ids: BTreeSet<u64>,
    // Used to extend |unused_track_ids| when it's empty.
    lowest_never_used: u64,
}

impl TrackIdAllocator {
    pub fn new() -> Self {
        Self {
            unused_track_ids: BTreeSet::new(),
            lowest_never_used: 0,
        }
    }

    /// Assign a track, unless we'd have > max tracks, in which case do nothing.
    fn assign_track(&mut self, max: Option<u64>) -> Option<u64> {
        let maybe_smallest = self.unused_track_ids.iter().next().copied();
        if let Some(n) = maybe_smallest {
            if let Some(max) = max {
                if max <= n {
                    return None;
                }
            }

            self.unused_track_ids.remove(&n);
            Some(n)
        } else {
            let n = self.lowest_never_used;
            if let Some(max) = max {
                if max <= n {
                    return None;
                }
            }
            self.lowest_never_used += 1;
            Some(n)
        }
    }

    pub fn mark_unused(&mut self, tid: u64) {
        self.unused_track_ids.insert(tid);
    }

    // Some spans hard-code the track assignments they are on. We ensure we bump
    // `lowest_never_used` to encompass such assignments to ensure we output the
    // correct track sorting order.
    pub fn mark_used(&mut self, tid: u64) {
        self.lowest_never_used = self.lowest_never_used.max(tid + 1);
    }
}

struct SimpleCounters<T> {
    name: &'static str,
    // timeseries are flushed every BUCKET_DURATION, if any changed.
    next_flush: SystemTime,
    /// Stores the current value of each timeseries.
    /// Set to None when we output a zero, so we can save a bit of filesize
    /// by omitting them from the JSON output.
    counters: BuckMutMap<String, SimpleCounter<T>>,
    zero_value: T,
    trace_events: Vec<serde_json::Value>,
}

struct SimpleCounter<T> {
    value: T,
    /// Whether this counter is currently represented in the trace as implicitly zero by not being
    /// emitted.
    implicitly_zero: bool,
}

impl<T> SimpleCounters<T>
where
    T: std::ops::SubAssign
        + std::cmp::PartialEq
        + std::ops::AddAssign
        + std::marker::Copy
        + Serialize,
{
    const BUCKET_DURATION: Duration = Duration::from_millis(100);
    pub fn new(name: &'static str, zero_value: T) -> Self {
        Self {
            name,
            next_flush: SystemTime::UNIX_EPOCH,
            counters: BuckMutMap::default(),
            trace_events: vec![],
            zero_value,
        }
    }

    /// Process the given timestamp and flush if needed and update next_flush accordingly
    fn process_timestamp(&mut self, timestamp: SystemTime) -> buck2_error::Result<()> {
        if self.next_flush == SystemTime::UNIX_EPOCH {
            self.next_flush = timestamp + Self::BUCKET_DURATION;
        }
        if timestamp > self.next_flush + Self::BUCKET_DURATION {
            self.flush()?;
            self.next_flush = timestamp - Duration::from_micros(1);
        }
        Ok(())
    }

    /// If the given key is new to the map, initialize it to self.zero_value;
    fn counter_entry(&mut self, key: &str) -> &mut SimpleCounter<T> {
        self.counters
            .entry(key.to_owned())
            .or_insert_with(|| SimpleCounter {
                value: self.zero_value,
                implicitly_zero: false,
            })
    }

    fn set(&mut self, timestamp: SystemTime, key: &str, amount: T) -> buck2_error::Result<()> {
        self.process_timestamp(timestamp)?;
        let entry = self.counter_entry(key);
        entry.value = amount;
        Ok(())
    }

    fn bump(&mut self, timestamp: SystemTime, key: &str, amount: T) -> buck2_error::Result<()> {
        self.process_timestamp(timestamp)?;
        let entry = self.counter_entry(key);
        entry.value += amount;
        Ok(())
    }

    fn subtract(&mut self, timestamp: SystemTime, key: &str, amount: T) -> buck2_error::Result<()> {
        self.process_timestamp(timestamp)?;
        let entry = self.counter_entry(key);
        entry.value -= amount;
        Ok(())
    }

    fn flush(&mut self) -> buck2_error::Result<()> {
        // Output size optimization: omit counters that were previously, and still are, zero.
        let mut counters_to_zero = Vec::new();
        let mut counters_to_output = json!({});

        for (key, counter) in self.counters.iter_mut() {
            // TODO: With float counters this equality comparison seems sketchy.
            if counter.value == self.zero_value {
                // If the counter is currently at its zero value, then emit the zero once, and then
                // stop emitting this counter altogether.
                if !counter.implicitly_zero {
                    counters_to_output[key] = json!(counter.value);
                    counter.implicitly_zero = true;
                }
            } else {
                // If the counter isn't zero, then we want to avoid the renderer interpolating from
                // its last zero value, if any. So, if the counter was previously "zeroed" by not
                // emitting it we'll emit an extra event setting it to zero.
                if counter.implicitly_zero {
                    counter.implicitly_zero = false;
                    counters_to_zero.push(key.clone());
                }

                counters_to_output[key] = json!(counter.value);
            }
        }

        let ts = self
            .next_flush
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_micros() as u64;

        if !counters_to_zero.is_empty() {
            let counters_to_zero = counters_to_zero
                .into_iter()
                .map(|k| (k, json!(0)))
                .collect::<serde_json::Map<_, _>>();

            self.trace_events.push(json!(
                {
                    "name": self.name,
                    "pid": 0,
                    "tid": "counters",
                    "ph": "C",
                    "ts": ts - 1,
                    "args": counters_to_zero,
                }
            ));
        }

        self.trace_events.push(json!(
            {
                "name": self.name,
                "pid": 0,
                "tid": "counters",
                "ph": "C",
                "ts": ts,
                "args": counters_to_output,
            }
        ));
        self.next_flush += Self::BUCKET_DURATION;
        Ok(())
    }

    pub fn flush_all_to(&mut self, output: &mut Vec<serde_json::Value>) -> buck2_error::Result<()> {
        self.flush()?;
        output.append(&mut self.trace_events);
        Ok(())
    }
}

struct TimestampAndAmount {
    timestamp: SystemTime,
    amount: u64,
}

struct AverageRateOfChangeCounters {
    counters: SimpleCounters<u64>,
    previous_timestamp_and_amount_by_key: BuckMutMap<String, TimestampAndAmount>,
}

impl AverageRateOfChangeCounters {
    pub fn new(name: &'static str) -> Self {
        Self {
            previous_timestamp_and_amount_by_key: BuckMutMap::default(),
            counters: SimpleCounters::<u64>::new(name, 0),
        }
    }

    /// Set a rate-of-change series directly, for values that are already
    /// instantaneous rather than cumulative. This lets one counter group
    /// mix instantenous rate-of-change values with computed-from-derivitive
    /// ones.
    fn set(&mut self, timestamp: SystemTime, key: &str, amount: u64) -> buck2_error::Result<()> {
        self.counters.set(timestamp, key, amount)
    }

    fn set_average_rate_of_change_per_s(
        &mut self,
        timestamp: SystemTime,
        key: &str,
        amount: u64,
    ) -> buck2_error::Result<()> {
        // We only plot if there exists a previous item to compute the rate of change off of
        if let Some(previous) = self.previous_timestamp_and_amount_by_key.get(key) {
            let secs_since_last_datapoint =
                timestamp.duration_since(previous.timestamp)?.as_secs_f64();
            let value_change_since_last_datapoint = (amount - previous.amount) as f64;
            if secs_since_last_datapoint > 0.0 {
                self.counters.set(
                    timestamp,
                    key,
                    (value_change_since_last_datapoint / secs_since_last_datapoint) as u64,
                )?;
            }
        }
        self.previous_timestamp_and_amount_by_key
            .insert(key.to_owned(), TimestampAndAmount { timestamp, amount });

        Ok(())
    }
}

struct SpanCounters {
    counter: SimpleCounters<i32>,
    // Stores how current open spans contribute to counter values.
    open_spans: BuckMutMap<buck2_events::span::SpanId, (&'static str, i32)>,
}

impl SpanCounters {
    pub fn new(name: &'static str) -> Self {
        Self {
            counter: SimpleCounters::new(name, 0),
            open_spans: BuckMutMap::default(),
        }
    }

    fn bump_counter_while_span(
        &mut self,
        event: &BuckEvent,
        key: &'static str,
        amount: i32,
    ) -> buck2_error::Result<()> {
        self.open_spans
            .insert(event.span_id().unwrap(), (key, amount));
        self.counter.bump(event.timestamp(), key, amount)
    }

    fn handle_event_end(
        &mut self,
        _end: &buck2_data::SpanEndEvent,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        if let Some((key, value)) = self.open_spans.remove(&event.span_id().unwrap()) {
            self.counter.subtract(event.timestamp(), key, value)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct CgroupMemoryMax {
    anon_bytes: u64,
    total_bytes: u64,
}

impl CgroupMemoryMax {
    fn update(&mut self, anon_bytes: u64, total_bytes: u64) -> (u64, u64) {
        self.anon_bytes = self.anon_bytes.max(anon_bytes);
        self.total_bytes = self.total_bytes.max(total_bytes);
        (self.anon_bytes, self.total_bytes)
    }
}

/// Build phases summarized on the `build-phases` async track: for each phase, one
/// span from the first start to the last end of any matching span event. These are
/// coarse envelopes for categories with no defined phase boundaries — they show
/// where the bulk of each kind of work begins and ends, not exclusive intervals.
/// Order defines the bit index used in phase masks.
const BUILD_PHASES: [&str; 11] = [
    "load",
    "analysis",
    "analysis (dynamic)",
    "action",
    "action: compile",
    "action: link",
    "action: lto",
    "action: bolt",
    "action: cache query",
    "action: remote exec",
    "action: local exec",
];

/// Bitmask over `BUILD_PHASES` of the phases a span belongs to. A span can be in
/// several buckets (e.g. `action` and `action: link`).
fn build_phase_mask(data: &buck2_data::span_start_event::Data) -> u16 {
    use buck2_data::analysis_start::Target;
    use buck2_data::span_start_event::Data;

    let bit = |name: &str| -> u16 {
        1 << BUILD_PHASES
            .iter()
            .position(|phase| *phase == name)
            .expect("phase names in this function must appear in BUILD_PHASES")
    };

    match data {
        Data::Load(..) => bit("load"),
        Data::Analysis(analysis) => match &analysis.target {
            // Anon targets are promise-driven, so group them with dynamic analysis.
            Some(Target::AnonTarget(..) | Target::DynamicLambda(..)) => bit("analysis (dynamic)"),
            Some(Target::StandardTarget(..)) | None => bit("analysis"),
        },
        Data::DynamicLambda(..) => bit("analysis (dynamic)"),
        Data::ActionExecution(action) => {
            let mut mask = bit("action");
            if let Some(name) = &action.name {
                for (keyword, phase) in [
                    ("compile", "action: compile"),
                    ("link", "action: link"),
                    ("lto", "action: lto"),
                    ("bolt", "action: bolt"),
                ] {
                    // Match whole snake_case tokens: `cxx_link` is a link but
                    // `symlinked_dir` is not.
                    if name.category.split('_').any(|token| token == keyword) {
                        mask |= bit(phase);
                    }
                }
            }
            mask
        }
        Data::ExecutorStage(executor) => {
            use buck2_data::executor_stage_start::Stage;
            match &executor.stage {
                Some(Stage::CacheQuery(..)) => bit("action: cache query"),
                Some(Stage::Re(re)) => match &re.stage {
                    Some(buck2_data::re_stage::Stage::Execute(..)) => bit("action: remote exec"),
                    _ => 0,
                },
                Some(Stage::Local(local)) => match &local.stage {
                    Some(
                        buck2_data::local_stage::Stage::Execute(..)
                        | buck2_data::local_stage::Stage::WorkerExecute(..),
                    ) => bit("action: local exec"),
                    _ => 0,
                },
                _ => 0,
            }
        }
        _ => 0,
    }
}

struct ChromeTraceWriter {
    trace_events: Vec<serde_json::Value>,
    open_spans: BuckMutMap<buck2_events::span::SpanId, ChromeTraceOpenSpan>,
    invocation: Invocation,
    first_pass: ChromeTraceFirstPass,
    max_tracks: u64,
    span_counters: SpanCounters,
    unused_track_ids: BuckMutMap<SpanCategorization, TrackIdAllocator>,
    // Wrappers to contain values from InstantEvent.Data.Snapshot as a timeseries
    snapshot_counters: SimpleCounters<u64>,
    process_memory_counters: SimpleCounters<f64>,
    allprocs_memory_max: CgroupMemoryMax,
    forkserver_actions_memory_max: CgroupMemoryMax,
    rate_of_change_counters: AverageRateOfChangeCounters,
    // Aggregate counters derived from InstantEvent.Data.DiceStateSnapshot,
    // grouped under a single "dice" counter name.
    dice_counters: AverageRateOfChangeCounters,
    // Distribution stats per entry of BUILD_PHASES.
    build_phases: [BuildPhaseStats; BUILD_PHASES.len()],
    // First/last snapshot timestamp at which each DICE key type's counters
    // changed, for min/max envelopes on the "dice activity" track. Resolution
    // is the DiceStateSnapshot cadence (~500ms).
    dice_activity: BuckMutMap<String, (SystemTime, SystemTime)>,
    dice_prev_key_states: BuckMutMap<String, buck2_data::DiceKeyState>,
    // Phase memberships and start time of currently-open spans, applied to the
    // stats when the corresponding SpanEnd arrives.
    open_build_phase_spans: BuckMutMap<buck2_events::span::SpanId, (u16, SystemTime)>,
}

/// Where one build phase's work sits in time: the raw first-start/last-end
/// envelope, plus duration-weighted moments of the work distribution. Each
/// span's work is modeled as spread uniformly over its interval, so a span
/// contributes `dur * midpoint` to the mean and a `dur^2 / 12` term to the
/// variance. Offsets are signed f64 seconds relative to `base` — an arbitrary
/// origin near the data (the first start processed) that keeps squared terms
/// well within f64 precision. Spans processed out of log order can precede
/// `base`, giving negative offsets; the moments are origin-invariant.
#[derive(Default)]
struct BuildPhaseStats {
    envelope: Option<(SystemTime, SystemTime)>,
    base: Option<SystemTime>,
    span_count: u64,
    weight_sum: f64,
    weighted_mid_sum: f64,
    weighted_sq_sum: f64,
}

/// Signed seconds from `base` to `t`; negative when `t` precedes `base`.
fn signed_secs_since(base: SystemTime, t: SystemTime) -> f64 {
    match t.duration_since(base) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

impl BuildPhaseStats {
    fn record_start(&mut self, timestamp: SystemTime) {
        self.base.get_or_insert(timestamp);
        self.envelope = Some(match self.envelope {
            None => (timestamp, timestamp),
            Some((start, end)) => (start.min(timestamp), end.max(timestamp)),
        });
    }

    fn record_end(&mut self, start: SystemTime, end: SystemTime) {
        if let Some((_, envelope_end)) = self.envelope.as_mut() {
            *envelope_end = (*envelope_end).max(end);
        }
        self.span_count += 1;

        let Some(base) = self.base else { return };
        let offset = |t: SystemTime| signed_secs_since(base, t);
        let (start, end) = (offset(start), offset(end));
        let duration = end - start;
        if duration <= 0.0 {
            return;
        }
        let mid = (start + end) / 2.0;
        self.weight_sum += duration;
        self.weighted_mid_sum += duration * mid;
        self.weighted_sq_sum += duration * (mid * mid + duration * duration / 12.0);
    }

    /// Duration-weighted (mean, stddev) of the work distribution, as offsets in
    /// seconds from `base`. `None` when no span with nonzero duration was seen.
    fn moments(&self) -> Option<(f64, f64)> {
        if self.weight_sum <= 0.0 {
            return None;
        }
        let mean = self.weighted_mid_sum / self.weight_sum;
        let variance = (self.weighted_sq_sum / self.weight_sum - mean * mean).max(0.0);
        Some((mean, variance.sqrt()))
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Dupe, Debug, Display, Hash, PartialEq, Eq)]
enum SpanCategorization {
    #[display("critical-path")]
    CriticalPath = 0,
    #[display("detailed-critical-path")]
    DetailedCriticalPath = 1,
    #[display("detailed-slowest-path")]
    DetailedSlowestPath = 2,
    #[display("uncategorized")]
    Uncategorized = 3,
}

impl ChromeTraceWriter {
    const BYTES_PER_GIGABYTE: f64 = 1000000000.0;

    pub fn new(invocation: Invocation, first_pass: ChromeTraceFirstPass, max_tracks: u64) -> Self {
        Self {
            trace_events: vec![],
            open_spans: BuckMutMap::default(),
            invocation,
            first_pass,
            max_tracks,
            unused_track_ids: BuckMutMap::default(),
            span_counters: SpanCounters::new("spans"),
            snapshot_counters: SimpleCounters::<u64>::new("snapshot_counters", 0),
            process_memory_counters: SimpleCounters::<f64>::new("process_memory", 0.0),
            allprocs_memory_max: CgroupMemoryMax::default(),
            forkserver_actions_memory_max: CgroupMemoryMax::default(),
            rate_of_change_counters: AverageRateOfChangeCounters::new("rate_of_change_counters"),
            dice_counters: AverageRateOfChangeCounters::new("dice"),
            build_phases: Default::default(),
            dice_activity: BuckMutMap::default(),
            dice_prev_key_states: BuckMutMap::default(),
            open_build_phase_spans: BuckMutMap::default(),
        }
    }

    /// Emit one `dice activity` legacy async track: a parent envelope over all
    /// DICE activity with a min/max child slice per key type, bounding when
    /// that key type's snapshot counters were changing. Envelopes only — no
    /// per-key-type distribution stats — to keep the event count small.
    fn write_dice_activity(&mut self) -> buck2_error::Result<()> {
        let Some(parent) = self
            .dice_activity
            .values()
            .copied()
            .reduce(|(first, last), (f, l)| (first.min(f), last.max(l)))
        else {
            return Ok(());
        };

        let mut children: Vec<(&String, &(SystemTime, SystemTime))> =
            self.dice_activity.iter().collect();
        children.sort_by_key(|(_, (first, _))| *first);

        // "5." pins this track after the four ordinal-prefixed build-phase
        // tracks in Perfetto's lexicographic group ordering.
        const PARENT_NAME: &str = "5. dice activity";

        let counter_args = |state: &buck2_data::DiceKeyState| {
            json!({
                "finished": state.finished,
                "check_deps_finished": state.check_deps_finished,
                "compute_finished": state.compute_finished,
            })
        };
        let totals = self.dice_prev_key_states.values().fold(
            buck2_data::DiceKeyState::default(),
            |mut acc, s| {
                acc.finished += s.finished;
                acc.check_deps_finished += s.check_deps_finished;
                acc.compute_finished += s.compute_finished;
                acc
            },
        );

        let mut events = Vec::with_capacity(2 * children.len() + 2);
        events.push((
            PARENT_NAME.to_owned(),
            "b",
            parent.0,
            Some(counter_args(&totals)),
        ));
        for (key_type, (first, last)) in children {
            // The final cumulative counters for this key type (counters only
            // ever grow, so the value stored at its last change is the total).
            let args = self.dice_prev_key_states.get(key_type).map(counter_args);
            events.push(((*key_type).clone(), "b", *first, args));
            events.push(((*key_type).clone(), "e", *last, None));
        }
        events.push((PARENT_NAME.to_owned(), "e", parent.1, None));

        for (name, ph, timestamp, args) in events {
            let mut event = json!({
                "name": name,
                "cat": "dice-activity",
                "ph": ph,
                "id": 0,
                "pid": 0,
                "tid": "dice-activity",
                "ts": timestamp
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_micros() as u64,
            });
            if let Some(args) = args {
                event["args"] = args;
            }
            self.trace_events.push(event);
        }
        Ok(())
    }

    fn record_build_phase_start(
        &mut self,
        span_id: buck2_events::span::SpanId,
        mask: u16,
        timestamp: SystemTime,
    ) {
        self.open_build_phase_spans
            .insert(span_id, (mask, timestamp));
        for (idx, stats) in self.build_phases.iter_mut().enumerate() {
            if mask & (1 << idx) != 0 {
                stats.record_start(timestamp);
            }
        }
    }

    fn record_build_phase_end(&mut self, mask: u16, start: SystemTime, end: SystemTime) {
        for (idx, stats) in self.build_phases.iter_mut().enumerate() {
            if mask & (1 << idx) != 0 {
                stats.record_end(start, end);
            }
        }
    }

    /// Emit each non-empty phase as nested slices on its own legacy async track
    /// (Perfetto groups these by `(pid, cat, id)` under the process, e.g. in
    /// "Global Legacy Events"): the full first-start/last-end envelope, with
    /// duration-weighted `±3σ` and `±1σ` bands of the work distribution nested
    /// inside it. A phase whose bulk happens early with a long tail shows as a
    /// wide envelope with the σ bands packed to the left.
    fn write_build_phases(&mut self) -> buck2_error::Result<()> {
        // Inset children so nesting stays unambiguous even when a band clamps
        // to its parent's edge (same trick as CHILD_TIME_OFFSET above).
        const BAND_INSET: f64 = 1e-6;

        for (idx, stats) in self.build_phases.iter().enumerate() {
            let Some((envelope_start, envelope_end)) = stats.envelope else {
                continue;
            };
            let base = stats.base.expect("base is set whenever an envelope exists");
            let envelope = (
                0.0,
                envelope_end
                    .duration_since(envelope_start)
                    .unwrap_or(Duration::ZERO)
                    .as_secs_f64(),
            );
            // Zero or negative: `envelope_start` is the true minimum start,
            // which precedes `base` when starts were processed out of log
            // order.
            let base_offset = signed_secs_since(base, envelope_start);

            // Action sub-phases (`action: *`) render as envelope-only children
            // on the parent `action` track rather than tracks of their own.
            // Their envelopes partially overlap each other, which is safe on a
            // shared track because SliceTracker::End matches slices by name.
            let phase = BUILD_PHASES[idx];
            let is_action_child = phase.starts_with("action: ");
            let track_id = if is_action_child {
                BUILD_PHASES
                    .iter()
                    .position(|p| *p == "action")
                    .expect("BUILD_PHASES contains `action`")
            } else {
                idx
            };

            // The outermost slice names the async track group, and Perfetto
            // orders the groups lexicographically, so prefix an ordinal to pin
            // the display order (dice-activity takes the slot after these).
            let envelope_name = if is_action_child {
                phase.to_owned()
            } else {
                format!("{}. {phase}", track_id + 1)
            };

            // Bands as (name suffix, start, end) offsets in seconds from the
            // envelope start, innermost last, each clamped inside its parent.
            let mut bands: Vec<(String, f64, f64)> = vec![(envelope_name, envelope.0, envelope.1)];
            if let Some((mean, sigma)) = stats.moments()
                && !is_action_child
            {
                let mean = mean - base_offset;
                let mut parent = envelope;
                for (label, width) in [("±3σ", 3.0 * sigma), ("±1σ", sigma)] {
                    let band = (
                        (mean - width).max(parent.0 + BAND_INSET),
                        (mean + width).min(parent.1 - BAND_INSET),
                    );
                    if band.0 >= band.1 {
                        break;
                    }
                    bands.push((format!("{} {label}", BUILD_PHASES[idx]), band.0, band.1));
                    parent = band;
                }
            }

            let ts = |offset: f64| -> buck2_error::Result<u64> {
                Ok(envelope_start
                    .checked_add(Duration::from_secs_f64(offset))
                    .unwrap_or(envelope_start)
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_micros() as u64)
            };

            // Begins outermost-first, ends innermost-first, so slices nest.
            for (name, start, _) in &bands {
                self.trace_events.push(json!({
                    "name": name,
                    "cat": "build-phase",
                    "ph": "b",
                    "id": track_id,
                    "pid": 0,
                    "tid": "build-phases",
                    "ts": ts(*start)?,
                    "args": {
                        "span_count": stats.span_count,
                    },
                }));
            }
            for (name, _, end) in bands.iter().rev() {
                self.trace_events.push(json!({
                    "name": name,
                    "cat": "build-phase",
                    "ph": "e",
                    "id": track_id,
                    "pid": 0,
                    "tid": "build-phases",
                    "ts": ts(*end)?,
                }));
            }
        }
        Ok(())
    }

    fn set_cgroup_memory_counters(
        &mut self,
        timestamp: SystemTime,
        name: &str,
        anon_bytes: u64,
        total_bytes: u64,
        max_bytes: (u64, u64),
    ) -> buck2_error::Result<()> {
        let (max_anon_bytes, max_total_bytes) = max_bytes;
        for (counter, bytes) in [
            (format!("{name}_anon_gigabyte"), anon_bytes),
            (format!("{name}_max_anon_gigabyte"), max_anon_bytes),
            (format!("{name}_total_gigabyte"), total_bytes),
            (format!("{name}_max_total_gigabyte"), max_total_bytes),
        ] {
            self.process_memory_counters.set(
                timestamp,
                &counter,
                bytes as f64 / Self::BYTES_PER_GIGABYTE,
            )?;
        }
        Ok(())
    }

    fn mark_track_used(
        &mut self,
        track_key: SpanCategorization,
        track_id: u64,
    ) -> buck2_error::Result<TrackId> {
        self.unused_track_ids
            .entry(track_key)
            .or_insert_with(TrackIdAllocator::new)
            .mark_used(track_id);
        Ok(TrackId {
            track_key,
            track: track_id,
        })
    }

    fn assign_track_for_span(
        &mut self,
        track_key: SpanCategorization,
        event: Option<&BuckEvent>,
    ) -> buck2_error::Result<Option<SpanTrackAssignment>> {
        let parent_track_id = event
            .and_then(|event| event.parent_id)
            .and_then(|parent_id| {
                self.open_spans
                    .get(&parent_id)
                    .map(|open_span| open_span.track.get_track_id())
            });

        match parent_track_id {
            None => {
                let max = match track_key {
                    SpanCategorization::CriticalPath => None,
                    SpanCategorization::Uncategorized => Some(self.max_tracks),
                    SpanCategorization::DetailedCriticalPath => None,
                    SpanCategorization::DetailedSlowestPath => None,
                };

                let track = self
                    .unused_track_ids
                    .entry(track_key)
                    .or_insert_with(TrackIdAllocator::new)
                    .assign_track(max);

                let assignment =
                    track.map(|track| SpanTrackAssignment::Owned(TrackId { track_key, track }));

                Ok(assignment)
            }
            Some(track_id) => Ok(Some(SpanTrackAssignment::Inherited(track_id))),
        }
    }

    pub fn into_writer<W>(mut self, file: W) -> buck2_error::Result<()>
    where
        W: Write,
    {
        self.span_counters
            .counter
            .flush_all_to(&mut self.trace_events)?;
        self.snapshot_counters
            .flush_all_to(&mut self.trace_events)?;
        self.process_memory_counters
            .flush_all_to(&mut self.trace_events)?;
        self.rate_of_change_counters
            .counters
            .flush_all_to(&mut self.trace_events)?;
        self.dice_counters
            .counters
            .flush_all_to(&mut self.trace_events)?;
        self.write_build_phases()?;
        self.write_dice_activity()?;

        serde_json::to_writer(
            file,
            &json!({
                "traceEvents": self.trace_events
            }),
        )?;
        Ok(())
    }

    fn open_span(
        &mut self,
        event: &BuckEvent,
        span: ChromeTraceOpenSpan,
    ) -> buck2_error::Result<()> {
        self.open_spans.insert(event.span_id().unwrap(), span);
        Ok(())
    }

    fn open_named_span(
        &mut self,
        event: &BuckEvent,
        name: String,
        track_key: SpanCategorization,
    ) -> buck2_error::Result<()> {
        // Allocate this span to its parent's track or to a new track.
        let track = self.assign_track_for_span(track_key, Some(event))?;
        if let Some(track) = track {
            self.open_span(
                event,
                ChromeTraceOpenSpan {
                    name,
                    start: event.timestamp(),
                    process_id: 0,
                    track,
                    categories: vec!["buck2"],
                    args: json!({
                        "span_id": event.span_id(),
                    }),
                },
            )?;
        }

        Ok(())
    }

    fn handle_event(&mut self, event: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        match event.data() {
            buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                data: Some(start_data),
            }) => {
                let phase_mask = build_phase_mask(start_data);
                if phase_mask != 0
                    && let Some(span_id) = event.span_id()
                {
                    self.record_build_phase_start(span_id, phase_mask, event.timestamp());
                }

                let on_critical_path = event.span_id().is_some_and(|span_id| {
                    self.first_pass
                        .critical_path_span_ids
                        .contains(&span_id.into())
                });

                enum Categorization<'a> {
                    /// Show this node on a specific track
                    Show {
                        category: SpanCategorization,
                        name: Cow<'a, str>,
                    },
                    /// Show this node if its parent is being shown.
                    ShowIfParent { name: Cow<'a, str> },
                    /// Do not show this node.
                    Omit,
                }

                let categorization = match start_data {
                    buck2_data::span_start_event::Data::Command(_command) => Categorization::Show {
                        category: SpanCategorization::Uncategorized,
                        name: self.invocation.command_line_args.join(" ").into(),
                    },
                    buck2_data::span_start_event::Data::Analysis(analysis) => {
                        self.span_counters
                            .bump_counter_while_span(event, "analysis", 1)?;

                        let category = if on_critical_path {
                            Some(SpanCategorization::CriticalPath)
                        } else if self
                            .first_pass
                            .long_analyses
                            .contains(&event.span_id().unwrap())
                        {
                            Some(SpanCategorization::Uncategorized)
                        } else {
                            None
                        };

                        match category {
                            Some(category) => {
                                let name = format!(
                                    "analysis {}",
                                    display::display_analysis_target(
                                        analysis
                                            .target
                                            .as_ref()
                                            .expect("AnalysisStart event missing 'target' field"),
                                        TargetDisplayOptions::for_chrome_trace()
                                    )?,
                                );

                                Categorization::Show {
                                    category,
                                    name: name.into(),
                                }
                            }
                            None => Categorization::Omit,
                        }
                    }
                    buck2_data::span_start_event::Data::Load(eval) => {
                        self.span_counters
                            .bump_counter_while_span(event, "load", 1)?;

                        let category = if on_critical_path {
                            Some(SpanCategorization::CriticalPath)
                        } else if self
                            .first_pass
                            .long_loads
                            .contains(&event.span_id().unwrap())
                        {
                            Some(SpanCategorization::Uncategorized)
                        } else {
                            None
                        };

                        match category {
                            Some(category) => Categorization::Show {
                                category,
                                name: format!("load {}", eval.module_id).into(),
                            },
                            None => Categorization::Omit,
                        }
                    }
                    buck2_data::span_start_event::Data::LoadPackage(load_package) => {
                        let category = if on_critical_path {
                            Some(SpanCategorization::CriticalPath)
                        } else if self
                            .first_pass
                            .long_load_packages
                            .contains(&event.span_id().unwrap())
                        {
                            Some(SpanCategorization::Uncategorized)
                        } else {
                            None
                        };

                        match category {
                            Some(category) => Categorization::Show {
                                category,
                                name: format!("listing {}", load_package.path).into(),
                            },
                            None => Categorization::Omit,
                        }
                    }
                    buck2_data::span_start_event::Data::ActionExecution(action) => {
                        #[allow(clippy::if_same_then_else)]
                        let category = if self
                            .first_pass
                            .critical_path_action_keys
                            .contains(action.key.as_ref().unwrap())
                        {
                            Some(SpanCategorization::CriticalPath)
                        } else if on_critical_path {
                            Some(SpanCategorization::CriticalPath)
                        } else if self
                            .first_pass
                            .local_actions
                            .contains(&event.span_id().unwrap())
                        {
                            Some(SpanCategorization::Uncategorized)
                        } else {
                            None
                        };

                        match category {
                            Some(category) => {
                                let name = display::display_action_identity(
                                    action.key.as_ref(),
                                    action.name.as_ref(),
                                    TargetDisplayOptions::for_chrome_trace(),
                                )?;

                                Categorization::Show {
                                    category,
                                    name: name.into(),
                                }
                            }
                            None => Categorization::Omit,
                        }
                    }
                    buck2_data::span_start_event::Data::ExecutorStage(stage) => {
                        let name = stage
                            .stage
                            .as_ref()
                            .and_then(display::display_executor_stage);

                        match name {
                            Some(name) => {
                                self.span_counters.bump_counter_while_span(event, name, 1)?;
                                Categorization::ShowIfParent { name: name.into() }
                            }
                            None => Categorization::Omit,
                        }
                    }
                    buck2_data::span_start_event::Data::ReUpload(_) => {
                        let name = "re_upload";
                        self.span_counters.bump_counter_while_span(event, name, 1)?;
                        Categorization::ShowIfParent { name: name.into() }
                    }
                    buck2_data::span_start_event::Data::FinalMaterialization(..) => {
                        if on_critical_path {
                            Categorization::Show {
                                category: SpanCategorization::CriticalPath,
                                name: "materialization".into(),
                            }
                        } else {
                            Categorization::Omit
                        }
                    }
                    buck2_data::span_start_event::Data::FileWatcher(_file_watcher) => {
                        Categorization::Show {
                            category: SpanCategorization::CriticalPath,
                            name: "file_watcher_sync".into(),
                        }
                    }
                    _ if on_critical_path => Categorization::Show {
                        category: SpanCategorization::CriticalPath,
                        name: "<unknown>".into(),
                    },
                    _ => Categorization::Omit,
                };

                match categorization {
                    Categorization::Show { category, name } => {
                        self.open_named_span(event, name.into_owned(), category)?;
                    }
                    Categorization::ShowIfParent { name } => {
                        let parent_is_open = event
                            .parent_id()
                            .is_some_and(|id| self.open_spans.contains_key(&id));

                        if parent_is_open {
                            // Inherit the parent's track.
                            self.open_named_span(
                                event,
                                name.into_owned(),
                                SpanCategorization::Uncategorized,
                            )?;
                        }
                    }

                    Categorization::Omit => {}
                }
            }
            // Data field is oneof and `None` means the event is produced with newer version of `.proto` file
            // which added a variant which is not available in version used when compiling this program.
            buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent { data: None }) => {}
            buck2_data::buck_event::Data::SpanEnd(end) => self.handle_event_end(end, event)?,
            buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                data: Some(instant_data),
            }) => match instant_data {
                buck2_data::instant_event::Data::Snapshot(snapshot) => {
                    if let Some(buck2_rss) = snapshot.buck2_rss {
                        self.process_memory_counters.set(
                            event.timestamp(),
                            "rss_gigabyte",
                            (buck2_rss) as f64 / Self::BYTES_PER_GIGABYTE,
                        )?;
                    }
                    self.process_memory_counters.set(
                        event.timestamp(),
                        "max_rss_gigabyte",
                        (snapshot.buck2_max_rss) as f64 / Self::BYTES_PER_GIGABYTE,
                    )?;
                    if let Some(malloc_bytes_active) = snapshot.malloc_bytes_active {
                        self.process_memory_counters.set(
                            event.timestamp(),
                            "malloc_active_gigabyte",
                            (malloc_bytes_active) as f64 / Self::BYTES_PER_GIGABYTE,
                        )?;
                    }
                    if let Some(malloc_bytes_allocated) = snapshot.malloc_bytes_allocated {
                        self.process_memory_counters.set(
                            event.timestamp(),
                            "malloc_allocated_gigabyte",
                            (malloc_bytes_allocated) as f64 / Self::BYTES_PER_GIGABYTE,
                        )?;
                    }
                    if let Some(allprocs) = &snapshot.allprocs_cgroup {
                        let total = allprocs
                            .anon
                            .saturating_add(allprocs.file)
                            .saturating_add(allprocs.kernel);
                        let max = self.allprocs_memory_max.update(allprocs.anon, total);
                        self.set_cgroup_memory_counters(
                            event.timestamp(),
                            "allprocs",
                            allprocs.anon,
                            total,
                            max,
                        )?;
                    }
                    if let Some(forkserver_actions) = &snapshot.forkserver_actions_cgroup {
                        let total = forkserver_actions
                            .anon
                            .saturating_add(forkserver_actions.file)
                            .saturating_add(forkserver_actions.kernel);
                        let max = self
                            .forkserver_actions_memory_max
                            .update(forkserver_actions.anon, total);
                        self.set_cgroup_memory_counters(
                            event.timestamp(),
                            "forkserver_actions",
                            forkserver_actions.anon,
                            total,
                            max,
                        )?;
                    }
                    self.rate_of_change_counters
                        .set_average_rate_of_change_per_s(
                            event.timestamp(),
                            "average_user_cpu_in_usecs_per_s",
                            snapshot.buck2_user_cpu_us,
                        )?;
                    self.rate_of_change_counters
                        .set_average_rate_of_change_per_s(
                            event.timestamp(),
                            "average_system_cpu_in_usecs_per_s",
                            snapshot.buck2_system_cpu_us,
                        )?;
                    if let Some(cpu_usage_system) = snapshot.host_cpu_usage_system_ms {
                        self.rate_of_change_counters
                            .set_average_rate_of_change_per_s(
                                event.timestamp(),
                                "host_cpu_usage_system_in_msecs_per_s",
                                cpu_usage_system,
                            )?;
                    }
                    if let Some(cpu_usage_user) = snapshot.host_cpu_usage_user_ms {
                        self.rate_of_change_counters
                            .set_average_rate_of_change_per_s(
                                event.timestamp(),
                                "host_cpu_usage_user_in_msecs_per_s",
                                cpu_usage_user,
                            )?;
                    }
                    self.snapshot_counters.set(
                        event.timestamp(),
                        "deferred_materializer_queue_size",
                        snapshot.deferred_materializer_queue_size,
                    )?;
                    self.snapshot_counters.set(
                        event.timestamp(),
                        "blocking_executor_io_queue_size",
                        snapshot.blocking_executor_io_queue_size,
                    )?;
                    self.snapshot_counters.set(
                        event.timestamp(),
                        "blocking_executor_io_inline_in_flight",
                        snapshot.blocking_executor_io_inline_in_flight,
                    )?;
                    self.snapshot_counters.set(
                        event.timestamp(),
                        "blocking_executor_io_inline_waiting",
                        snapshot.blocking_executor_io_inline_waiting,
                    )?;
                    self.snapshot_counters.set(
                        event.timestamp(),
                        "tokio_blocking_queue_depth",
                        snapshot.tokio_blocking_queue_depth,
                    )?;
                    self.snapshot_counters.set(
                        event.timestamp(),
                        "tokio_num_blocking_threads",
                        snapshot.tokio_num_blocking_threads,
                    )?;
                    self.snapshot_counters.set(
                        event.timestamp(),
                        "tokio_num_idle_blocking_threads",
                        snapshot.tokio_num_idle_blocking_threads,
                    )?;
                    for (nic, stats) in &snapshot.network_interface_stats {
                        self.rate_of_change_counters
                            .set_average_rate_of_change_per_s(
                                event.timestamp(),
                                &format!("{}_send_bytes", nic),
                                stats.tx_bytes,
                            )?;
                        self.rate_of_change_counters
                            .set_average_rate_of_change_per_s(
                                event.timestamp(),
                                &format!("{}_receive_bytes", nic),
                                stats.rx_bytes,
                            )?;
                    }
                    self.rate_of_change_counters
                        .set_average_rate_of_change_per_s(
                            event.timestamp(),
                            "re_upload_bytes",
                            snapshot.re_upload_bytes,
                        )?;
                    self.rate_of_change_counters
                        .set_average_rate_of_change_per_s(
                            event.timestamp(),
                            "re_download_bytes",
                            snapshot.re_download_bytes,
                        )?;
                    self.rate_of_change_counters
                        .set_average_rate_of_change_per_s(
                            event.timestamp(),
                            "http_download_bytes",
                            snapshot.http_download_bytes,
                        )?;
                }
                buck2_data::instant_event::Data::DiceStateSnapshot(dice) => {
                    let mut total_check_deps: u64 = 0;
                    let mut total_computes: u64 = 0;
                    for (key_type, state) in &dice.key_states {
                        total_check_deps += state.check_deps_finished as u64;
                        total_computes += state.compute_finished as u64;
                        if self.dice_prev_key_states.get(key_type) != Some(state) {
                            self.dice_prev_key_states.insert(key_type.clone(), *state);
                            let timestamp = event.timestamp();
                            self.dice_activity
                                .entry(key_type.clone())
                                .and_modify(|(_, last)| *last = timestamp)
                                .or_insert((timestamp, timestamp));
                        }
                    }
                    self.dice_counters.set_average_rate_of_change_per_s(
                        event.timestamp(),
                        "check_deps_per_s",
                        total_check_deps,
                    )?;
                    self.dice_counters.set_average_rate_of_change_per_s(
                        event.timestamp(),
                        "computes_per_s",
                        total_computes,
                    )?;
                    self.dice_counters.set(
                        event.timestamp(),
                        "core_state_queue_depth",
                        dice.core_state_queue_depth,
                    )?;
                    self.dice_counters.set_average_rate_of_change_per_s(
                        event.timestamp(),
                        "core_state_processed_per_s",
                        dice.core_state_processed_requests,
                    )?;
                }
                buck2_data::instant_event::Data::ResourceControlEvent(events) => {
                    self.snapshot_counters.set(
                        event.timestamp(),
                        "allprocs_memory_pressure",
                        events.allprocs_memory_pressure,
                    )?
                }
                buck2_data::instant_event::Data::CommandPreempted(..) => {
                    self.trace_events.push(
                        ChromeTraceInstant {
                            name: "command_preempted".to_owned(),
                            timestamp: event.timestamp(),
                            scope: ChromeTraceInstantScope::Global,
                            args: None,
                        }
                        .into_json()?,
                    );
                }
                _ => {}
            },
            // Data field is oneof and `None` means the event is produced with newer version of `.proto` file
            // which added a variant which is not available in version used when compiling this program.
            buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent { data: None }) => {}
            buck2_data::buck_event::Data::Record(_) => {}
        };
        Ok(())
    }

    fn write_instant_events(&mut self, events: Vec<ChromeTraceInstant>) -> buck2_error::Result<()> {
        self.trace_events.reserve(events.len());
        for event in events.into_iter() {
            self.trace_events.push(event.into_json()?);
        }
        Ok(())
    }

    fn write_thread_names(&mut self) -> buck2_error::Result<()> {
        for (track_key, allocator) in self.unused_track_ids.iter() {
            for track in 0..allocator.lowest_never_used {
                let track_id = TrackId {
                    track_key: *track_key,
                    track,
                };
                let tid = track_id.as_u64()?;
                let event = ChromeTraceMetadata {
                    kind: ChromeTraceMetadataKind::Thread { pid: 0, tid },
                    name: String::from(track_id),
                    labels: None,
                    sort_index: Some(tid),
                };
                self.trace_events.push(event.into_json()?);
            }
        }
        Ok(())
    }

    fn write_critical_path(
        &mut self,
        track_key: SpanCategorization,
        critical_path: &[buck2_data::CriticalPathEntry2],
    ) -> buck2_error::Result<()> {
        // Write critical path as a series of spans on a dedicated track
        let target_display_options = TargetDisplayOptions::for_chrome_trace();
        self.write_critical_path_hierarchical(track_key, critical_path, target_display_options)
    }

    /// Write the critical path with hierarchical structure:
    /// - Non-waiting entries become parent spans
    /// - Preceding waiting entries become child spans (nested within parent)
    /// - The execution time of the non-waiting entry becomes a "critical execution time" child span
    fn write_critical_path_hierarchical(
        &mut self,
        name: SpanCategorization,
        critical_path: &[buck2_data::CriticalPathEntry2],
        target_display_options: TargetDisplayOptions,
    ) -> buck2_error::Result<()> {
        use buck2_data::critical_path_entry2::Entry;

        // All spans go on track 1. Parent-child relationships are determined by
        // time containment in Chrome trace format.
        const TRACK: u64 = 1;

        // Collect entries into groups: each group is [Waiting*, NonWaiting]
        // where the waiting entries are associated with the following non-waiting entry.
        let mut pending_waiting: Vec<&buck2_data::CriticalPathEntry2> = Vec::new();

        for entry in critical_path {
            let is_waiting = matches!(&entry.entry, Some(Entry::Waiting(_)));

            if is_waiting {
                pending_waiting.push(entry);
            } else {
                // This is a non-waiting entry - render the group
                self.write_critical_path_group(
                    name,
                    &pending_waiting,
                    entry,
                    target_display_options,
                    TRACK,
                )?;
                pending_waiting.clear();
            }
        }

        // Handle any trailing waiting entries by creating a synthetic parent
        if !pending_waiting.is_empty() {
            // Create a synthetic GenericEntry to act as the parent for orphaned waiting entries.
            // The parent span needs to cover all waiting entries, so we set start_offset_ns
            // to the last waiting entry's start and total_duration to its duration.
            // This way the parent span will cover from first_waiting.start to last_waiting.end.
            let last_waiting = pending_waiting.last().unwrap();

            let synthetic_parent = buck2_data::CriticalPathEntry2 {
                span_ids: Vec::new(),
                duration: last_waiting.total_duration,
                user_duration: None,
                total_duration: last_waiting.total_duration,
                potential_improvement_duration: None,
                queue_duration: None,
                non_critical_path_duration: last_waiting.non_critical_path_duration,
                start_offset_ns: last_waiting.start_offset_ns,
                entry: Some(Entry::GenericEntry(
                    buck2_data::critical_path_entry2::GenericEntry {
                        kind: "waiting".to_owned(),
                    },
                )),
                was_reused: None,
            };

            self.write_critical_path_group(
                name,
                &pending_waiting,
                &synthetic_parent,
                target_display_options,
                TRACK,
            )?;
        }

        Ok(())
    }

    /// Write a single group: parent span covering all entries, with child spans for
    /// waiting entries and the execution time.
    fn write_critical_path_group(
        &mut self,
        name: SpanCategorization,
        waiting_entries: &[&buck2_data::CriticalPathEntry2],
        main_entry: &buck2_data::CriticalPathEntry2,
        target_display_options: TargetDisplayOptions,
        track: u64,
    ) -> buck2_error::Result<()> {
        // Calculate the overall start time and duration for the parent span
        let first_start_offset = waiting_entries.first().map_or_else(
            || main_entry.start_offset_ns.unwrap_or(0),
            |e| e.start_offset_ns.unwrap_or(0),
        );

        let parent_start_time = self
            .first_pass
            .command_start
            .checked_add(Duration::from_nanos(first_start_offset))
            .unwrap();

        // Calculate the end time from the main entry
        let main_start_offset = main_entry.start_offset_ns.unwrap_or(0);
        let main_critical_duration = main_entry
            .total_duration
            .as_ref()
            .and_then(|d| d.try_into_duration().ok())
            .unwrap_or(Duration::ZERO);
        let main_non_critical_duration = main_entry
            .non_critical_path_duration
            .as_ref()
            .and_then(|d| d.try_into_duration().ok())
            .unwrap_or(Duration::ZERO);
        let main_total_duration = main_critical_duration + main_non_critical_duration;

        let parent_end_offset_ns = main_start_offset + main_total_duration.as_nanos() as u64;
        let parent_duration = Duration::from_nanos(parent_end_offset_ns - first_start_offset);

        if parent_duration < Duration::from_millis(1) {
            return Ok(());
        }

        // Get display info for the main entry (this becomes the parent name)
        let main_display =
            CriticalPathEntryDisplay::from_entry(main_entry, target_display_options)?;
        let main_display = match main_display {
            Some(display) => display,
            None => return Ok(()),
        };

        // Build args for parent span
        let mut parent_args = serde_json::Map::new();
        parent_args.insert("kind".to_owned(), json!(main_display.kind));
        if main_display.reused {
            parent_args.insert("reused".to_owned(), json!(true));
        }
        if !main_display.name.is_empty() {
            parent_args.insert("name".to_owned(), json!(main_display.name));
        }
        if let Some(category) = main_display.category {
            parent_args.insert("category".to_owned(), json!(category));
        }
        if let Some(identifier) = main_display.identifier {
            parent_args.insert("identifier".to_owned(), json!(identifier));
        }
        if let Some(execution_kind) = main_display.execution_kind {
            parent_args.insert("execution_kind".to_owned(), json!(execution_kind));
        }

        let parent_name = main_display.display_name();

        // Create the track ID for this group - parent owns it, children inherit it
        let track_id = self.mark_track_used(name, track)?;

        // Write parent span (owns the track)
        self.trace_events.push(
            ChromeTraceClosedSpan {
                open: ChromeTraceOpenSpan {
                    name: parent_name,
                    start: parent_start_time,
                    process_id: 0,
                    track: SpanTrackAssignment::Owned(track_id),
                    categories: vec![],
                    args: parent_args.into(),
                },
                duration: parent_duration,
            }
            .into_json()?,
        );

        // Small offset to avoid trace viewer rendering issues when parent/child
        // share exact start/end times. Chrome trace uses microseconds.
        const CHILD_TIME_OFFSET: Duration = Duration::from_micros(1);

        // Write child spans for waiting entries (inherit the track from parent)
        // Offset start time by 1 unit to avoid exact overlap with parent start
        for waiting_entry in waiting_entries {
            self.write_critical_path_child_entry(
                waiting_entry,
                target_display_options,
                track_id,
                CHILD_TIME_OFFSET,
                Duration::ZERO, // no end offset for waiting entries
            )?;
        }

        // Write "critical execution time" child span for the main entry's execution
        let main_start_time = self
            .first_pass
            .command_start
            .checked_add(Duration::from_nanos(main_start_offset))
            .unwrap();

        if main_total_duration >= Duration::from_millis(1) {
            // Offset start by 1 unit and reduce duration by 1 unit so it ends before parent
            let adjusted_start = main_start_time + CHILD_TIME_OFFSET;
            let adjusted_duration = main_total_duration.saturating_sub(CHILD_TIME_OFFSET * 2);

            if adjusted_duration >= Duration::from_millis(1) {
                let exec_name = format!("{}: execution", main_display.kind);
                self.trace_events.push(
                    ChromeTraceClosedSpan {
                        open: ChromeTraceOpenSpan {
                            name: exec_name,
                            start: adjusted_start,
                            process_id: 0,
                            track: SpanTrackAssignment::Inherited(track_id),
                            categories: vec![],
                            args: json!({"kind": "execution"}),
                        },
                        duration: adjusted_duration,
                    }
                    .into_json()?,
                );
            }
        }

        Ok(())
    }

    /// Write a child critical path entry as a span (inherits track from parent).  start_offset and
    /// end_offset are used to slightly shrink the child span to avoid exact overlap with parent
    /// boundaries (as trace viewers have inconsistent behavior if child spans aren't strictly
    /// nested in the parent).
    fn write_critical_path_child_entry(
        &mut self,
        entry: &buck2_data::CriticalPathEntry2,
        target_display_options: TargetDisplayOptions,
        parent_track_id: TrackId,
        start_offset: Duration,
        end_offset: Duration,
    ) -> buck2_error::Result<()> {
        let start_time = self
            .first_pass
            .command_start
            .checked_add(Duration::from_nanos(entry.start_offset_ns.unwrap_or(0)))
            .unwrap()
            + start_offset;
        let critical_duration = entry
            .total_duration
            .as_ref()
            .and_then(|d| d.try_into_duration().ok())
            .unwrap_or(Duration::ZERO);
        let non_critical_duration = entry
            .non_critical_path_duration
            .as_ref()
            .and_then(|d| d.try_into_duration().ok())
            .unwrap_or(Duration::ZERO);
        let duration = (critical_duration + non_critical_duration)
            .saturating_sub(start_offset)
            .saturating_sub(end_offset);

        if duration < Duration::from_millis(1) {
            return Ok(());
        }

        let entry_display = CriticalPathEntryDisplay::from_entry(entry, target_display_options)?;
        let entry_display = match entry_display {
            Some(display) => display,
            None => return Ok(()),
        };

        let mut args = serde_json::Map::new();
        args.insert("kind".to_owned(), json!(entry_display.kind));
        if entry_display.reused {
            args.insert("reused".to_owned(), json!(true));
        }
        if !entry_display.name.is_empty() {
            args.insert("name".to_owned(), json!(entry_display.name));
        }

        let entry_name = entry_display.display_name();

        self.trace_events.push(
            ChromeTraceClosedSpan {
                open: ChromeTraceOpenSpan {
                    name: entry_name,
                    start: start_time,
                    process_id: 0,
                    track: SpanTrackAssignment::Inherited(parent_track_id),
                    categories: vec![],
                    args: args.into(),
                },
                duration,
            }
            .into_json()?,
        );

        Ok(())
    }

    fn handle_event_end(
        &mut self,
        end: &buck2_data::SpanEndEvent,
        event: &BuckEvent,
    ) -> buck2_error::Result<()> {
        self.span_counters.handle_event_end(end, event)?;
        if let Some((mask, start)) = self
            .open_build_phase_spans
            .remove(&event.span_id().unwrap())
        {
            self.record_build_phase_end(mask, start, event.timestamp());
        }
        if let Some(open) = self.open_spans.remove(&event.span_id().unwrap()) {
            let duration = end
                .duration
                .as_ref()
                .internal_error("Expected SpanEndEvent to have duration")?
                .try_into_duration()?;
            if let SpanTrackAssignment::Owned(track_id) = &open.track {
                self.unused_track_ids
                    .get_mut(&track_id.track_key)
                    .unwrap()
                    .mark_unused(track_id.track);
            }
            self.trace_events
                .push(ChromeTraceClosedSpan { open, duration }.into_json()?);
        }

        match end.data.as_ref() {
            Some(buck2_data::span_end_event::Data::Materialization(materialization)) => {
                if !materialization.success {
                    self.trace_events.push(
                        ChromeTraceInstant {
                            name: "materialization_failure".to_owned(),
                            timestamp: event.timestamp(),
                            // These have parent_id == 0, so we can't relate
                            // them back to the action that cause them right now
                            scope: ChromeTraceInstantScope::Global,
                            args: Some(json!({
                                "file_count": materialization.file_count,
                                "total_bytes": materialization.total_bytes,
                                "path": materialization.path,
                                "action_digest": materialization.action_digest.as_ref().map(|digest| digest.to_string()),
                                "success": materialization.success,
                                "error": materialization.error,
                                // "method": materialization.method, // TODO: convert to string?
                            })),
                        }
                        .into_json()?,
                    );
                }
            }
            _ => {}
        }

        Ok(())
    }
}

impl ChromeTraceCommand {
    pub fn exec(
        self,
        matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        ctx.exec(self, matches, events_ctx)
    }

    async fn load_events(
        log_path: EventLogPathBuf,
    ) -> buck2_error::Result<(
        Invocation,
        BoxStream<'static, buck2_error::Result<BuckEvent>>,
    )> {
        let (invocation, stream_values) = log_path.unpack_stream().await?;
        let stream = stream_values.try_filter_map(|stream_value| async move {
            match stream_value {
                StreamValue::Event(e) => Ok(Some(BuckEvent::try_from(e)?)),
                _ => Ok(None),
            }
        });

        Ok((invocation, Box::pin(stream)))
    }

    fn trace_path_from_dir(
        dir: AbsPathBuf,
        log: &std::path::Path,
    ) -> buck2_error::Result<AbsPathBuf> {
        match log.file_name() {
            None => Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Could not determine filename from event log path: `{:#}`",
                log.display()
            )),
            Some(file_name) => {
                let mut trace_path = dir;
                trace_path.push(file_name);
                trace_path.set_extension("trace");
                Ok(trace_path)
            }
        }
    }
}

impl BuckSubcommand for ChromeTraceCommand {
    const COMMAND_NAME: &'static str = "chrome-trace";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        // For backward compatibility, use the path field if it's set
        let log = if let Some(path) = self.event_log_path {
            EventLogPathBuf::infer(path.resolve(&ctx.working_dir))?
        } else {
            self.event_log.get(&ctx).await?
        };

        #[cfg(fbcode_build)]
        let (trace_path, _temp_trace_file) = match (self.output.trace_path, self.output.upload) {
            (Some(trace_path), _) => (trace_path.resolve(&ctx.working_dir), None),
            (None, false) => {
                return ExitResult::err(buck2_error::internal_error!(
                    "clap should have required at least one of --trace-path/--upload"
                ));
            }
            (None, true) => {
                let temp_trace_file = tempfile::NamedTempFile::new()?;
                (
                    ctx.working_dir.resolve(temp_trace_file.path()),
                    Some(temp_trace_file),
                )
            }
        };
        #[cfg(not(fbcode_build))]
        let trace_path = self.output.trace_path.resolve(&ctx.working_dir);

        let dest_path = if trace_path.is_dir() {
            Self::trace_path_from_dir(trace_path, log.path())
                .buck_error_context("Could not determine trace path")?
        } else {
            trace_path
        };

        let mut instant_events = Vec::with_capacity(self.instant.len() / 2);
        let (instant_args, instant_remainder) = self.instant.as_chunks::<2>();
        if !instant_remainder.is_empty() {
            return ExitResult::err(buck2_error::internal_error!(
                "Expected even number of arguments for --instant"
            ));
        }
        for instant in instant_args {
            let event = Self::parse_marker_arg(&instant[0], &instant[1])?;
            instant_events.push(event);
        }

        let writer = Self::trace_writer(log, self.max_tracks, instant_events).await?;
        #[cfg(fbcode_build)]
        let trace_id = writer.invocation.trace_id.clone();

        let tracefile = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&dest_path)?;
        let mut enc = GzEncoder::new(tracefile, Compression::default());
        writer.into_writer(&mut enc)?;
        drop(enc);

        #[cfg(fbcode_build)]
        if self.output.upload {
            let bucket = buck2_common::manifold::Bucket::EVENT_LOGS;
            let sys_info = buck2_events::metadata::system_info();
            let username = sys_info
                .username
                .unwrap_or_else(|| "unknown_user".to_owned());
            let timestamp = jiff::Timestamp::now()
                .strftime("%Y-%m-%dT%H:%M:%S%.3f%:z")
                .to_string();

            let manifold_filename =
                format!("flat/{trace_id}_{username}_{timestamp}.chrome_trace.gz");
            println!("Uploading {manifold_filename}...");
            let client = buck2_common::manifold::ManifoldClient::new().await?;
            let explorer_url = client
                .upload_file(
                    &dest_path,
                    manifold_filename.clone(),
                    bucket,
                    buck2_common::manifold::Ttl::from_days(30),
                )
                .await?;
            fn ansi_url(url: &str, text: &str) -> String {
                const ESC: &str = "\x1b";
                const ESCURL: &str = const_format::concatcp!(ESC, "]8;;");
                const ESCSEP: &str = const_format::concatcp!(ESC, "\\");
                format!("{ESCURL}{url}{ESCSEP}{text}{ESCURL}{ESCSEP}")
            }
            let download_url = bucket.intern_url(manifold_filename.as_str());
            println!(
                "Uploaded generated trace: {}",
                ansi_url(&explorer_url, &explorer_url)
            );
            println!(
                "Direct download: {}",
                ansi_url(&download_url, &download_url)
            );

            const PERFETTO_URL: &str = "https://www.internalfb.com/intern/perfetto/open_trace/";
            let mut query_string = form_urlencoded::Serializer::new(String::new());
            query_string.append_pair("manifold_path", &bucket.path(manifold_filename.as_str()));
            let query_string = query_string.finish();
            let perfetto_url = format!("{PERFETTO_URL}?{query_string}");

            println!("Perfetto: {}", ansi_url(&perfetto_url, &perfetto_url));
        }

        ExitResult::success()
    }
}

impl ChromeTraceCommand {
    fn parse_marker_arg(time: &str, name: &str) -> buck2_error::Result<ChromeTraceInstant> {
        // Look for integer or float times. Note that we don't parse f64 in order to maintain the tv_nsec values
        let datetime = buck2_event_log::utils::timestamp::parse(time)?;
        Ok(ChromeTraceInstant {
            name: name.to_owned(),
            timestamp: datetime.into(),
            scope: ChromeTraceInstantScope::Global,
            args: None,
        })
    }

    async fn trace_writer(
        log: EventLogPathBuf,
        max_tracks: Option<u64>,
        instant_events: Vec<ChromeTraceInstant>,
    ) -> buck2_error::Result<ChromeTraceWriter> {
        let (invocation, mut stream) = Self::load_events(log.clone()).await?;
        let mut first_pass = ChromeTraceFirstPass::new();
        let mut build_graph_info = None;
        while let Some(event) = tokio_stream::StreamExt::try_next(&mut stream).await? {
            first_pass
                .handle_event(&event)
                .with_buck_error_context(|| {
                    display::InvalidBuckEvent(Arc::new(event.clone())).to_string()
                })?;
            if let Ok(UnpackedBuckEvent::Instant(
                _,
                _,
                buck2_data::instant_event::Data::BuildGraphInfo(info),
            )) = unpack_event(&event)
            {
                build_graph_info = Some(info.clone());
            }
        }

        let max_tracks = max_tracks.unwrap_or(
            first_pass
                .command_options
                .map_or(20, |opt| opt.configured_parallelism),
        );

        let mut writer = ChromeTraceWriter::new(invocation, first_pass, max_tracks);

        // We do this to ensure that these are the first two tracks.
        if let Some(info) = build_graph_info {
            if !info.slowest_path.is_empty() {
                writer.write_critical_path(
                    SpanCategorization::DetailedSlowestPath,
                    &info.slowest_path,
                )?;
            }
            if !info.critical_path2.is_empty() {
                writer.write_critical_path(
                    SpanCategorization::DetailedCriticalPath,
                    &info.critical_path2,
                )?;
            }
        }

        writer.write_instant_events(instant_events)?;

        // We just read events again from log file, in order to avoid holding all logs in memory
        let (_invocation, mut stream) = Self::load_events(log).await?;
        while let Some(event) = tokio_stream::StreamExt::try_next(&mut stream).await? {
            let event = Arc::new(event);
            writer
                .handle_event(&event)
                .with_buck_error_context(|| display::InvalidBuckEvent(event).to_string())?;
        }

        writer.write_thread_names()?;

        Ok(writer)
    }
}

#[cfg(test)]
mod tests {
    use buck2_events::span::SpanId;
    use buck2_wrapper_common::invocation_id::TraceId;
    use clap::Parser;

    use super::*;

    #[test]
    fn test_the_trace_path_is_given_with_a_flag() {
        let command =
            ChromeTraceCommand::try_parse_from(["chrome-trace", "--trace-path", "trace.json"]);
        assert!(command.is_ok(), "{command:?}");
    }

    /// Renders `spans` overlapping top-level command spans with `max_tracks` and returns the
    /// distinct tracks the written JSON uses.
    fn rendered_tracks(max_tracks: u64, spans: u64) -> Vec<u64> {
        let trace_id = TraceId::new();
        let invocation = Invocation {
            command_line_args: vec!["buck2".to_owned(), "build".to_owned()],
            expanded_command_line_args: vec![],
            working_dir: "/".to_owned(),
            trace_id: trace_id.clone(),
            start_time: None,
        };
        let mut writer =
            ChromeTraceWriter::new(invocation, ChromeTraceFirstPass::new(), max_tracks);
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for i in 1..=spans {
            let start = BuckEvent::new(
                t0 + Duration::from_millis(i),
                trace_id.clone(),
                Some(SpanId::from_u64(i).unwrap()),
                None,
                buck2_data::SpanStartEvent {
                    data: Some(buck2_data::span_start_event::Data::Command(
                        buck2_data::CommandStart::default(),
                    )),
                }
                .into(),
            );
            writer.handle_event(&Arc::new(start)).unwrap();
        }
        for i in 1..=spans {
            let end = BuckEvent::new(
                t0 + Duration::from_secs(10),
                trace_id.clone(),
                Some(SpanId::from_u64(i).unwrap()),
                None,
                buck2_data::SpanEndEvent {
                    duration: Some(Duration::from_secs(9).try_into().unwrap()),
                    data: Some(buck2_data::span_end_event::Data::Command(
                        buck2_data::CommandEnd::default(),
                    )),
                    ..Default::default()
                }
                .into(),
            );
            writer.handle_event(&Arc::new(end)).unwrap();
        }
        let mut out = Vec::new();
        writer.into_writer(&mut out).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let mut tids: Vec<u64> = json["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["ph"] == "X")
            .map(|e| e["tid"].as_u64().unwrap())
            .collect();
        tids.sort();
        tids.dedup();
        tids
    }

    /// `--max-tracks N` renders at most N tracks.
    #[test]
    fn test_max_tracks_bounds_the_rendered_tracks() {
        assert_eq!(rendered_tracks(1, 2).len(), 1);
        assert_eq!(rendered_tracks(0, 1).len(), 0);
        assert_eq!(rendered_tracks(2, 2).len(), 2);
    }
}
