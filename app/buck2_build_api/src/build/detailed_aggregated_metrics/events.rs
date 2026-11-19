/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use buck2_artifact::actions::key::ActionKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::get_dispatcher_opt;
use dupe::Dupe;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::build::detailed_aggregated_metrics::implementation::state::DetailedAggregatedMetricsStateTracker;
use crate::build::detailed_aggregated_metrics::types::ActionExecutionMetrics;
use crate::build::detailed_aggregated_metrics::types::ActionGraphSketchResult;
use crate::build::detailed_aggregated_metrics::types::DetailedAggregatedMetrics;
use crate::build::detailed_aggregated_metrics::types::DetailedMetricsInput;
use crate::build::detailed_aggregated_metrics::types::PerBuildEvents;
use crate::build::detailed_aggregated_metrics::types::TopLevelTargetSpec;
use crate::deferred::calculation::DeferredHolder;
use crate::deferred::calculation::OwnedDeferredHolder;

pub(crate) enum DetailedAggregatedMetricsEvent {
    AnalysisStarted(DeferredHolderKey),
    AnalysisComplete(DeferredHolderKey, OwnedDeferredHolder),
    ComputeMetrics(
        DetailedMetricsInput,
        tokio::sync::oneshot::Sender<buck2_error::Result<DetailedAggregatedMetrics>>,
        Option<EventDispatcher>,
    ),
    ComputeActionGraphSketch(
        Vec<TopLevelTargetSpec>,
        tokio::sync::oneshot::Sender<buck2_error::Result<ActionGraphSketchResult>>,
        Option<EventDispatcher>,
    ),
    ActionExecuted(ActionExecutionMetrics),
}

struct DetailedAggregatedMetricsEventHandlerInner {
    sender: UnboundedSender<DetailedAggregatedMetricsEvent>,
}

/// Handle to the running tracker task: a thin wrapper over the channel used to
/// send it events.
#[derive(Clone, Dupe)]
pub(crate) struct DetailedAggregatedMetricsEventHandler(
    Arc<DetailedAggregatedMetricsEventHandlerInner>,
);

impl DetailedAggregatedMetricsEventHandler {
    pub(crate) fn new() -> (Self, UnboundedReceiver<DetailedAggregatedMetricsEvent>) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            Self(Arc::new(DetailedAggregatedMetricsEventHandlerInner {
                sender,
            })),
            receiver,
        )
    }

    fn send(&self, event: DetailedAggregatedMetricsEvent) -> buck2_error::Result<()> {
        self.0.sender.send(event).map_err(|_| {
            internal_error!("DetailedAggregatedMetrics event handler exited while sender lives")
        })?;
        Ok(())
    }

    async fn compute_metrics(
        &self,
        events: DetailedMetricsInput,
    ) -> buck2_error::Result<DetailedAggregatedMetrics> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(DetailedAggregatedMetricsEvent::ComputeMetrics(
            events,
            tx,
            get_dispatcher_opt(),
        ))?;
        rx.await?
    }

    async fn compute_action_graph_sketch(
        &self,
        top_level_targets: Vec<TopLevelTargetSpec>,
    ) -> buck2_error::Result<ActionGraphSketchResult> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(DetailedAggregatedMetricsEvent::ComputeActionGraphSketch(
            top_level_targets,
            tx,
            get_dispatcher_opt(),
        ))?;
        rx.await?
    }
}

/// Daemon-global handle stored in DICE global data (so must be `Send + Sync`); the
/// single entry point for recording analyses/actions and computing metrics.
///
/// The tracker is spawned lazily by the first [`Self::enable`], so daemons that
/// never collect metrics retain nothing. Once enabled it stays enabled for the
/// daemon's lifetime.
#[derive(Default)]
pub struct DetailedAggregatedMetricsHandle {
    tracker: OnceLock<DetailedAggregatedMetricsEventHandler>,
    /// Set when an analysis is observed before the tracker is enabled. Those
    /// analyses may later be reused from the DICE cache without recomputing, so
    /// the tracker's view would be permanently incomplete; the compute methods
    /// then return empty results.
    analysis_nodes_not_recorded: AtomicBool,
}

impl DetailedAggregatedMetricsHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn the tracker if not already running (idempotent). Reports a soft
    /// error if analyses already ran while disabled, since metrics will be
    /// incomplete; enabling on the daemon's first command (or first build command)
    /// avoids this.
    pub fn enable(&self) {
        if self.tracker.get().is_some() {
            return;
        }
        if self.analysis_nodes_not_recorded.load(Ordering::Relaxed) {
            buck2_core::soft_error_or_panic!(
                "detailed_aggregated_metrics_enabled_after_analysis",
                buck2_error!(
                    ErrorTag::Tier0,
                    "Detailed aggregated metrics / action graph sketch were enabled after analyses \
                     already ran on this daemon; collected metrics may be incomplete for analyses \
                     reused from earlier commands. Enable the config on the daemon's first command."
                )
            );
        }
        let _ = self
            .tracker
            .get_or_init(DetailedAggregatedMetricsStateTracker::start);
    }

    pub fn action_executed(&self, ev: ActionExecutionMetrics) -> buck2_error::Result<()> {
        match self.tracker.get() {
            Some(tracker) => tracker.send(DetailedAggregatedMetricsEvent::ActionExecuted(ev)),
            None => Ok(()),
        }
    }

    pub fn analysis_started(&self, key: &DeferredHolderKey) -> buck2_error::Result<()> {
        match self.tracker.get() {
            Some(tracker) => {
                tracker.send(DetailedAggregatedMetricsEvent::AnalysisStarted(key.dupe()))
            }
            None => {
                self.analysis_nodes_not_recorded
                    .store(true, Ordering::Relaxed);
                Ok(())
            }
        }
    }

    pub fn analysis_complete(
        &self,
        key: &DeferredHolderKey,
        result: &DeferredHolder<'_>,
    ) -> buck2_error::Result<()> {
        match self.tracker.get() {
            Some(tracker) => tracker.send(DetailedAggregatedMetricsEvent::AnalysisComplete(
                key.dupe(),
                result.to_owned_holder(),
            )),
            None => Ok(()),
        }
    }

    pub(crate) async fn compute_metrics(
        &self,
        events: DetailedMetricsInput,
    ) -> buck2_error::Result<DetailedAggregatedMetrics> {
        // Incomplete tracker view: report empty rather than partial metrics.
        if self.analysis_nodes_not_recorded.load(Ordering::Relaxed) {
            return Ok(DetailedAggregatedMetrics::default());
        }
        match self.tracker.get() {
            Some(tracker) => tracker.compute_metrics(events).await,
            None => Err(internal_error!(
                "should have had a detailed aggregated metrics event holder"
            )),
        }
    }

    pub async fn compute_action_graph_sketch(
        &self,
        top_level_targets: Vec<TopLevelTargetSpec>,
    ) -> buck2_error::Result<ActionGraphSketchResult> {
        // Incomplete tracker view: report empty rather than partial sketch.
        if self.analysis_nodes_not_recorded.load(Ordering::Relaxed) {
            return Ok(ActionGraphSketchResult::default());
        }
        match self.tracker.get() {
            Some(tracker) => tracker.compute_action_graph_sketch(top_level_targets).await,
            None => Ok(ActionGraphSketchResult::default()),
        }
    }
}

pub enum PerBuildEvent {
    ActionExecuted(ActionKey),
    TopLevelTarget(TopLevelTargetSpec),
}

pub(crate) struct DetailedAggregatedMetricsPerBuildEventsHolder {
    events: Mutex<Vec<PerBuildEvent>>,
}

impl DetailedAggregatedMetricsPerBuildEventsHolder {
    pub(crate) fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn action_executed(&self, key: &ActionKey) {
        self.events
            .lock()
            .unwrap()
            .push(PerBuildEvent::ActionExecuted(key.dupe()));
    }

    pub(crate) fn top_level_target(&self, spec: TopLevelTargetSpec) {
        self.events
            .lock()
            .unwrap()
            .push(PerBuildEvent::TopLevelTarget(spec));
    }

    pub(crate) fn take_events(&self) -> buck2_error::Result<PerBuildEvents> {
        let mut events = PerBuildEvents::default();
        let messages = { std::mem::take(&mut *self.events.lock().unwrap()) };

        for ev in messages {
            match ev {
                PerBuildEvent::ActionExecuted(key) => {
                    events.executed_actions.insert(key);
                }
                PerBuildEvent::TopLevelTarget(spec) => {
                    events.top_level_targets.push(spec);
                }
            }
        }

        Ok(events)
    }
}
