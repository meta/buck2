/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Records each executed action's direct producers in the event log, for `--log-action-graph`.

use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use buck2_artifact::actions::key::ActionKey;
use buck2_data::action_graph_footer::Incomplete;
use buck2_data::timing::Micros;
use buck2_events::dispatch::instant_event;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use dice::DiceComputations;
use dice::DiceTransaction;
use dice::UserComputationData;
use dupe::Dupe;
use futures::FutureExt;

use crate::actions::impls::run_action_knobs::HasRunActionKnobs;
use crate::artifact_groups::ArtifactGroup;

/// The schema of recorded action graphs.
const ACTION_GRAPH_SCHEMA: u32 = 1;

/// The action graph recorder's state for one command.
#[derive(Clone, Dupe, Default)]
pub struct GraphRecorder(Arc<RecorderState>);

#[derive(Default)]
struct RecorderState {
    /// The span id of the `ActionExecution` event of each action that ran, so that consumers can
    /// refer to their producers by span.
    producer_spans: Mutex<BuckMutMap<ActionKey, u64>>,
    /// A per-command id for each reused producer that the graph refers to, numbered from 0 in
    /// order of first reference.
    reused_ids: Mutex<BuckMutMap<ActionKey, u64>>,
    action_ends: AtomicU64,
    capture: TotalTime,
}

/// A time that threads add to without a lock. It is kept exact, and rounded only when written.
#[derive(Default)]
struct TotalTime {
    nanos: AtomicU64,
}

impl TotalTime {
    fn add(&self, d: Duration) {
        self.nanos.fetch_add(
            u64::try_from(d.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    fn get(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::Relaxed))
    }
}

impl GraphRecorder {
    pub(crate) fn record(&self, action: &ActionKey, span_id: u64) {
        self.0
            .producer_spans
            .lock()
            .expect("poisoned lock")
            .insert(action.dupe(), span_id);
    }

    /// Counts an action-end event that was written with graph fields.
    pub(crate) fn action_end_written(&self) {
        self.0.action_ends.fetch_add(1, Ordering::Relaxed);
    }

    fn start_event() -> buck2_data::ActionGraphStart {
        buck2_data::ActionGraphStart {
            schema: ACTION_GRAPH_SCHEMA,
        }
    }

    fn footer(&self, incomplete: Option<Incomplete>) -> buck2_data::ActionGraphFooter {
        buck2_data::ActionGraphFooter {
            schema: ACTION_GRAPH_SCHEMA,
            incomplete: incomplete.map(|i| i as i32),
            action_ends: self.0.action_ends.load(Ordering::Relaxed),
            capture_us: Micros::from(self.0.capture.get()).into(),
            tset_def_events: 0,
            definitions_walk_us: 0,
            structure_defs: 0,
            aliases: 0,
        }
    }
}

/// An action's direct producers, as recorded on its `ActionExecutionEnd`.
#[derive(Debug, Default)]
pub(crate) struct DirectProducers {
    /// Producers that ran in this command, by the span id of their `ActionExecution` event.
    span_ids: Vec<u64>,
    /// Producers that didn't run in this command, because their results were reused, by per-command
    /// id.
    reused_ids: Vec<u64>,
}

impl DirectProducers {
    /// Finds the producers of `inputs`, skipping inputs that arrive through a transitive set.
    pub(crate) fn collect<'a>(
        recorder: &GraphRecorder,
        inputs: impl IntoIterator<Item = &'a ArtifactGroup>,
    ) -> Self {
        let start = Instant::now();
        let keys = producer_keys(inputs);
        let mut result = DirectProducers::default();
        if !keys.is_empty() {
            let mut reused = Vec::new();
            let spans = recorder.0.producer_spans.lock().expect("poisoned lock");
            for key in keys {
                match spans.get(key) {
                    Some(span_id) => result.span_ids.push(*span_id),
                    None => reused.push(key),
                }
            }
            drop(spans);
            if !reused.is_empty() {
                let mut ids = recorder.0.reused_ids.lock().expect("poisoned lock");
                result.reused_ids = reused
                    .into_iter()
                    .map(|key| {
                        let next = ids.len() as u64;
                        *ids.entry(key.dupe()).or_insert(next)
                    })
                    .collect();
                result.reused_ids.sort_unstable();
            }
        }
        result.span_ids.sort_unstable();
        recorder.0.capture.add(Instant::now() - start);
        result
    }

    pub(crate) fn into_proto(self) -> buck2_data::ActionGraphProducers {
        buck2_data::ActionGraphProducers {
            span_ids: self.span_ids,
            reused_ids: self.reused_ids,
            tset_ids: Vec::new(),
        }
    }
}

/// The producers of `inputs` by action key, leaving out inputs that arrive through a transitive
/// set.
fn producer_keys<'a>(
    inputs: impl IntoIterator<Item = &'a ArtifactGroup>,
) -> BuckMutSet<&'a ActionKey> {
    let mut keys = BuckMutSet::<&ActionKey>::default();
    for input in inputs {
        match input {
            ArtifactGroup::Artifact(artifact) => keys.extend(artifact.action_key()),
            ArtifactGroup::Promise(promise) => keys.extend(
                promise
                    .promise_artifact
                    .get()
                    .and_then(|artifact| artifact.action_key()),
            ),
            ArtifactGroup::TransitiveSetProjection(_) => {}
        }
    }
    keys
}

/// Runs a command's `work`, recording the action graph around it if this command records one.
/// `incomplete` says why a successful result still leaves the graph incomplete, or `None`; an
/// error always marks it `BuildFailed`. The start event is written when this is called, not when
/// the returned future is first polled.
pub fn with_graph_recording<T, W, F>(
    ctx: DiceTransaction,
    work: W,
    incomplete: F,
) -> impl Future<Output = buck2_error::Result<T>>
where
    W: Future<Output = buck2_error::Result<T>>,
    F: FnOnce(&T) -> Option<Incomplete>,
{
    // `work` is not awaited inside an `async fn` or `async` block, which would hold it twice and
    // double the future's size. See https://github.com/rust-lang/rust/issues/62958
    let recording = start_recording(&ctx.ctx());
    work.map(move |result| {
        if recording {
            let incomplete = match &result {
                Ok(value) => incomplete(value),
                Err(_) => Some(Incomplete::BuildFailed),
            };
            finish_recording(&ctx.ctx(), incomplete);
        }
        result
    })
}

/// Writes the start event if this command records the action graph, and returns whether it does.
fn start_recording(ctx: &DiceComputations<'_>) -> bool {
    let recording = ctx
        .per_transaction_data()
        .get_run_action_knobs()
        .log_action_graph;
    if recording {
        instant_event(GraphRecorder::start_event());
    }
    recording
}

/// Ends a command that records the action graph: writes the footer.
fn finish_recording(ctx: &DiceComputations<'_>, incomplete: Option<Incomplete>) {
    instant_event(
        ctx.per_transaction_data()
            .get_graph_recorder()
            .footer(incomplete),
    );
}

pub trait HasGraphRecorder {
    fn set_graph_recorder(&mut self, recorder: GraphRecorder);

    fn get_graph_recorder(&self) -> &GraphRecorder;
}

impl HasGraphRecorder for UserComputationData {
    fn set_graph_recorder(&mut self, recorder: GraphRecorder) {
        self.data.set(recorder);
    }

    fn get_graph_recorder(&self) -> &GraphRecorder {
        self.data
            .get::<GraphRecorder>()
            .expect("GraphRecorder should be set")
    }
}

#[cfg(test)]
mod tests {
    use buck2_artifact::actions::key::ActionIndex;
    use buck2_artifact::artifact::artifact_type::Artifact;
    use buck2_artifact::artifact::artifact_type::testing::BuildArtifactTestingExt;
    use buck2_artifact::artifact::build_artifact::BuildArtifact;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;

    use super::*;

    fn build_artifact(index: u32) -> ArtifactGroup {
        let target =
            ConfiguredTargetLabel::testing_parse("cell//pkg:t", ConfigurationData::testing_new());
        ArtifactGroup::Artifact(Artifact::from(BuildArtifact::testing_new(
            target,
            &format!("out{index}"),
            ActionIndex::new(index),
        )))
    }

    fn key(input: &ArtifactGroup) -> ActionKey {
        let ArtifactGroup::Artifact(artifact) = input else {
            unreachable!()
        };
        artifact.action_key().unwrap().dupe()
    }

    #[test]
    fn test_no_inputs_allocates_nothing() {
        let producers = DirectProducers::collect(&GraphRecorder::default(), []);
        assert_eq!(producers.span_ids.capacity(), 0);
        assert_eq!(producers.reused_ids.capacity(), 0);
    }

    #[test]
    fn test_footer_counts() {
        let recorder = GraphRecorder::default();
        let empty = recorder.footer(None);
        assert_eq!(
            (empty.schema, empty.action_ends, empty.incomplete),
            (ACTION_GRAPH_SCHEMA, 0, None)
        );

        recorder.action_end_written();
        recorder.action_end_written();
        let footer = recorder.footer(Some(
            buck2_data::action_graph_footer::Incomplete::BuildFailed,
        ));
        assert_eq!(footer.action_ends, 2);
        assert_eq!(
            footer.incomplete,
            Some(buck2_data::action_graph_footer::Incomplete::BuildFailed as i32)
        );
    }

    #[test]
    fn test_capture_time_is_rounded_once_when_written() {
        let recorder = GraphRecorder::default();
        for _ in 0..10 {
            recorder.0.capture.add(Duration::from_nanos(1_500));
        }
        assert_eq!(
            recorder.footer(None).capture_us,
            15,
            "ten 1.5 µs captures make 15 µs; rounding each to whole microseconds would make 10"
        );
    }

    #[test]
    fn test_producers_by_span_or_by_reused_id() {
        let (ran, reused, other) = (build_artifact(1), build_artifact(2), build_artifact(3));
        let recorder = GraphRecorder::default();
        recorder.record(&key(&ran), 7);

        let producers = DirectProducers::collect(&recorder, [&ran, &reused, &ran]);
        assert_eq!(producers.span_ids, vec![7], "ran in this command, once");
        assert_eq!(
            producers.reused_ids,
            vec![0],
            "reused from an earlier command, by id"
        );
        let next = DirectProducers::collect(&recorder, [&other, &reused]);
        assert_eq!(
            next.reused_ids,
            vec![0, 1],
            "the same producer keeps its id, and a new one gets the next"
        );
    }
}
