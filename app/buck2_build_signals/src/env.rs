/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::str::FromStr;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_error::BuckErrorContext;
use buck2_events::dispatch::EventDispatcher;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_hash::IntentionallyStdHashMap;
use buck2_util::early_command_timing::EarlyCommandTiming;
use buck2_util::time_span::TimeSpan;
use dice::UserComputationData;
use dupe::Dupe;
use variant_name::VariantName;

use crate::error::CriticalPathError;

#[derive(Copy, Clone, Dupe)]
pub struct NodeDuration {
    /// The amount of time for this node that corresponds to something the user might be able to
    /// improve. We should better break this down.
    pub user: Duration,
    /// The total duration for this node.
    pub total: TimeSpan,
    /// The waiting duration for this node.
    pub queue: Option<Duration>,
}

impl NodeDuration {
    /// Returns the duration we are using in our critical path calculation. This doesn't really
    /// *need* to be a function but is helpful so that not every callsite has to know which one we chose.
    pub fn critical_path_duration(&self) -> Duration {
        self.total.duration()
    }

    pub fn zero() -> Self {
        Self {
            user: Duration::from_secs(0),
            total: TimeSpan::from_start_and_duration(Instant::now(), Duration::ZERO),
            queue: None,
        }
    }
}

/// Data about time spent waiting (not on the critical path) during build execution.
/// This will be enriched with specific waiting categories and time spans to provide
/// better visibility into non-critical path time.
#[derive(Clone, Debug)]
pub struct WaitingData(Option<Box<WaitingDataInitialized>>);

#[derive(Clone, Debug)]
struct WaitingDataInitialized {
    /// Category of the time before the first entry of `categorized_waiting`.
    initial_category: WaitingCategory,
    categorized_waiting: Vec<(Instant, WaitingCategory)>,
}

impl WaitingData {
    pub fn new() -> Self {
        Self(None)
    }

    /// Waiting data whose time before the first recorded category, back to when the node's
    /// dependencies finished, is attributed to `category` rather than `Unknown`.
    pub fn with_initial_category(category: WaitingCategory) -> Self {
        Self(Some(Box::new(WaitingDataInitialized {
            initial_category: category,
            categorized_waiting: Vec::new(),
        })))
    }

    /// Returns an iterator over categorized waiting time spans, starting at `start_time` and ending at `end_time`.
    pub fn iter_spans<'a>(
        &'a self,
        start_time: Instant,
        end_time: Instant,
    ) -> impl Iterator<Item = (TimeSpan, &'a WaitingCategory)> {
        match &self.0 {
            Some(initialized) if !initialized.categorized_waiting.is_empty() => {
                let mut spans = Vec::with_capacity(1 + initialized.categorized_waiting.len());
                spans.push((
                    TimeSpan::new_saturating(start_time, initialized.categorized_waiting[0].0),
                    &initialized.initial_category,
                ));
                for (i, (start, category)) in initialized.categorized_waiting.iter().enumerate() {
                    let end = initialized
                        .categorized_waiting
                        .get(i + 1)
                        .map_or(end_time, |x| x.0);

                    spans.push((TimeSpan::new_saturating(*start, end), category));
                }

                spans.into_iter()
            }
            Some(initialized) => vec![(
                TimeSpan::new_saturating(start_time, end_time),
                &initialized.initial_category,
            )]
            .into_iter(),
            None => vec![(
                TimeSpan::new_saturating(start_time, end_time),
                &WaitingCategory::Unknown,
            )]
            .into_iter(),
        }
    }

    /// Records the start of a new waiting category phase.
    /// Tracks timing boundaries between different phases of action execution.
    pub fn start_waiting_category_now(&mut self, category: WaitingCategory) {
        self.start_waiting_category_at(category, Instant::now());
    }

    fn start_waiting_category_at(&mut self, category: WaitingCategory, start_time: Instant) {
        self.init().categorized_waiting.push((start_time, category));
    }

    fn init(&mut self) -> &mut WaitingDataInitialized {
        self.0.get_or_insert_with(|| {
            Box::new(WaitingDataInitialized {
                initial_category: WaitingCategory::Unknown,
                categorized_waiting: Vec::new(),
            })
        })
    }
}

impl Default for WaitingData {
    fn default() -> Self {
        Self::new()
    }
}

/// Categories for classifying time spent waiting during build execution.
/// Used to categorize non-critical path time for better build performance insights.
#[derive(VariantName, Clone, Dupe, Debug, Allocative, Eq, PartialEq)]
pub enum WaitingCategory {
    Unknown,
    /// Time spent preparing action inputs and command-line arguments.
    PreparingAction,
    /// Time spent checking action cache and dep file caches.
    CheckingCaches,
    MaterializingInputs,
    LocalQueued,
    MaterializerPrepare,
    MaterializerStage2,
    /// Time between a test's dependencies finishing and the test executor asking buck2 to run
    /// it: the executor's own scheduling, which includes waiting for an executor slot.
    TestExecutorScheduling,
    /// Time between an install's dependencies finishing and the installer being launched and
    /// connected.
    InstallerStartup,
    /// Time a `run` action spent waiting for an identical action already in flight
    /// (`buck2.dedupe_in_flight_actions`).
    InFlight,
}

#[derive(Copy, Clone, Dupe, derive_more::Display, Allocative)]
pub enum CriticalPathBackendName {
    /// This is the default backend.
    #[display("longest-path-graph")]
    LongestPathGraph,
    #[display("logging")]
    Logging,
}

impl FromStr for CriticalPathBackendName {
    type Err = buck2_error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "longest-path-graph" {
            return Ok(Self::LongestPathGraph);
        } else if s == "logging" {
            return Ok(Self::Logging);
        }

        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Invalid backend name: `{}`",
            s
        ))
    }
}

pub struct BuildSignalsContext {
    pub command_name: String,
    pub metadata: IntentionallyStdHashMap<String, String>,
    pub isolation_prefix: FileNameBuf,
    pub early_command_timing: EarlyCommandTiming,
}

/// Created along with the BuildSignalsInstaller (ideally, BuildSignalsInstaller's definition would
/// live here, but that can't be done for now because it has some dependencies on buck2_build_api).
///
/// This can be started to actually start processing build signals.
pub trait DeferredBuildSignals: Send {
    fn start(
        self: Box<Self>,
        events: EventDispatcher,
        backend: CriticalPathBackendName,
        ctx: BuildSignalsContext,
    ) -> Box<dyn FinishBuildSignals>;
}

/// Returned by DeferredBuildSignals once started. Lets us report that we finished.
#[async_trait]
pub trait FinishBuildSignals: Send {
    async fn finish(self: Box<Self>) -> Result<(), CriticalPathError>;
}

/// Start the backend for a DeferredBuildSignals instance.
///
/// Build listeners operate by creating a matched pair of signal senders and signal receivers.
/// Senders are Dupe and allow for arbitrarily many writeres. Receivers are not Dupe and are
/// expected to be driven by a single thread. This implies that, in order for the receiver to
/// function correctly and dispatch to build listeners, it must be run in a background task that is
/// periodically polled.
///
/// This function arranges for a background task to be spawned that drives the receiver, while
/// invoking the called function with a live BuildSignalSender that can be used to send events to
/// the listening receiver. Upon return of `scope`, the sender terminates the receiver by sending a
/// `BuildFinished` signal and joins the receiver task.
pub async fn scope<F, R, Fut>(
    deferred: Box<dyn DeferredBuildSignals>,
    events: EventDispatcher,
    backend: CriticalPathBackendName,
    ctx: BuildSignalsContext,
    func: F,
) -> buck2_error::Result<R>
where
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = buck2_error::Result<R>> + Send,
    R: Send,
{
    let handle = deferred.start(events, backend, ctx);
    let result = func().await;
    if let Err(e) = handle
        .finish()
        .await
        .buck_error_context("Error computing critical path")
    {
        buck2_core::soft_error!("critical_path_computation_failed", e);
    }
    result
}

pub trait HasCriticalPathBackend {
    fn set_critical_path_backend(&mut self, backend: CriticalPathBackendName);

    fn get_critical_path_backend(&self) -> CriticalPathBackendName;
}

impl HasCriticalPathBackend for UserComputationData {
    fn set_critical_path_backend(&mut self, backend: CriticalPathBackendName) {
        self.data.set(backend);
    }

    fn get_critical_path_backend(&self) -> CriticalPathBackendName {
        *self
            .data
            .get::<CriticalPathBackendName>()
            .expect("CriticalPathBackendName should be set")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iter_spans_empty_waiting_data() {
        let waiting_data = WaitingData::new();
        let base = Instant::now();
        let start_time = base;
        let end_time = base + Duration::from_secs(10);

        let spans: Vec<_> = waiting_data.iter_spans(start_time, end_time).collect();

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, &WaitingCategory::Unknown);
        assert_eq!(spans[0].0.duration(), Duration::from_secs(10));
    }

    #[test]
    fn test_iter_spans_initial_category() {
        let base = Instant::now();
        let end_time = base + Duration::from_secs(10);

        let waiting_data =
            WaitingData::with_initial_category(WaitingCategory::TestExecutorScheduling);
        let spans: Vec<_> = waiting_data.iter_spans(base, end_time).collect();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, &WaitingCategory::TestExecutorScheduling);
        assert_eq!(spans[0].0.duration(), Duration::from_secs(10));

        let mut waiting_data =
            WaitingData::with_initial_category(WaitingCategory::TestExecutorScheduling);
        waiting_data.start_waiting_category_at(
            WaitingCategory::PreparingAction,
            base + Duration::from_secs(7),
        );
        let spans: Vec<_> = waiting_data.iter_spans(base, end_time).collect();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].1, &WaitingCategory::TestExecutorScheduling);
        assert_eq!(spans[0].0.duration(), Duration::from_secs(7));
        assert_eq!(spans[1].1, &WaitingCategory::PreparingAction);
        assert_eq!(spans[1].0.duration(), Duration::from_secs(3));
    }

    #[test]
    fn test_iter_spans_single_category() {
        let base = Instant::now();
        let start_time = base;
        let category_start = base + Duration::from_secs(2);
        let end_time = base + Duration::from_secs(10);

        let mut waiting_data = WaitingData::new();
        waiting_data.start_waiting_category_at(WaitingCategory::PreparingAction, category_start);

        let spans: Vec<_> = waiting_data.iter_spans(start_time, end_time).collect();

        assert_eq!(spans.len(), 2);

        // First span: Unknown from start_time to category_start
        assert_eq!(spans[0].1, &WaitingCategory::Unknown);
        assert_eq!(spans[0].0.duration(), Duration::from_secs(2));

        // Second span: PreparingAction from category_start to end_time
        assert_eq!(spans[1].1, &WaitingCategory::PreparingAction);
        assert_eq!(spans[1].0.duration(), Duration::from_secs(8));
    }

    #[test]
    fn test_iter_spans_multiple_categories() {
        let base = Instant::now();
        let start_time = base;
        let category1_start = base + Duration::from_secs(1);
        let category2_start = base + Duration::from_secs(4);
        let category3_start = base + Duration::from_secs(7);
        let end_time = base + Duration::from_secs(10);

        let mut waiting_data = WaitingData::new();
        waiting_data.start_waiting_category_at(WaitingCategory::PreparingAction, category1_start);
        waiting_data.start_waiting_category_at(WaitingCategory::CheckingCaches, category2_start);
        waiting_data
            .start_waiting_category_at(WaitingCategory::MaterializingInputs, category3_start);

        let spans: Vec<_> = waiting_data.iter_spans(start_time, end_time).collect();

        assert_eq!(spans.len(), 4);

        // First span: Unknown from start_time to category1_start (1 sec)
        assert_eq!(spans[0].1, &WaitingCategory::Unknown);
        assert_eq!(spans[0].0.duration(), Duration::from_secs(1));

        // Second span: PreparingAction from category1_start to category2_start (3 secs)
        assert_eq!(spans[1].1, &WaitingCategory::PreparingAction);
        assert_eq!(spans[1].0.duration(), Duration::from_secs(3));

        // Third span: CheckingCaches from category2_start to category3_start (3 secs)
        assert_eq!(spans[2].1, &WaitingCategory::CheckingCaches);
        assert_eq!(spans[2].0.duration(), Duration::from_secs(3));

        // Fourth span: MaterializingInputs from category3_start to end_time (3 secs)
        assert_eq!(spans[3].1, &WaitingCategory::MaterializingInputs);
        assert_eq!(spans[3].0.duration(), Duration::from_secs(3));
    }

    #[test]
    fn test_iter_spans_category_at_start() {
        let base = Instant::now();
        let start_time = base;
        let end_time = base + Duration::from_secs(5);

        let mut waiting_data = WaitingData::new();
        // Category starts exactly at start_time
        waiting_data.start_waiting_category_at(WaitingCategory::LocalQueued, start_time);

        let spans: Vec<_> = waiting_data.iter_spans(start_time, end_time).collect();

        assert_eq!(spans.len(), 2);

        // First span: Unknown with zero duration (start_time to start_time)
        assert_eq!(spans[0].1, &WaitingCategory::Unknown);
        assert_eq!(spans[0].0.duration(), Duration::ZERO);

        // Second span: LocalQueued from start_time to end_time
        assert_eq!(spans[1].1, &WaitingCategory::LocalQueued);
        assert_eq!(spans[1].0.duration(), Duration::from_secs(5));
    }
}
