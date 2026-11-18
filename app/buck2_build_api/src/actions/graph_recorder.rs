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

use std::collections::VecDeque;
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
use crate::artifact_groups::TransitiveSetProjectionKey;

/// The schema of recorded action graphs.
const ACTION_GRAPH_SCHEMA: u32 = 1;

/// A transitive-set definition bigger than this is split across several events.
///
/// Nothing enforces a limit (these events go only to the client and the event log); splitting
/// only stops one huge set from becoming an event every log reader must decode whole. Real sets
/// are far smaller: in recorded builds of buck2 and PyTorch (470K sets), the biggest was 125 KB
/// and 99.9% were under 4 KB.
const TSET_DEF_BUDGET_BYTES: usize = 4 << 20;

/// Characters of a projection key kept for display.
const DISPLAY_KEY_CHARS: usize = 200;

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
    /// A per-command id for each transitive-set projection that an action referred to, numbered
    /// from 0 in order of first reference.
    tset_ids: Mutex<BuckMutMap<TransitiveSetProjectionKey, u64>>,
    action_ends: AtomicU64,
    capture: TotalTime,
    tset_def_events: AtomicU64,
    definitions_walk: TotalTime,
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

    /// The per-command id of `key`, assigning the next one if it has none.
    fn tset_id(&self, key: &TransitiveSetProjectionKey) -> u64 {
        let mut ids = self.0.tset_ids.lock().expect("poisoned lock");
        let next = ids.len() as u64;
        *ids.entry(key.dupe()).or_insert(next)
    }

    /// The transitive-set projections that actions referred to, with their ids.
    fn referenced_tsets(&self) -> Vec<(TransitiveSetProjectionKey, u64)> {
        self.0
            .tset_ids
            .lock()
            .expect("poisoned lock")
            .iter()
            .map(|(key, id)| (key.dupe(), *id))
            .collect()
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
            tset_def_events: self.0.tset_def_events.load(Ordering::Relaxed),
            definitions_walk_us: Micros::from(self.0.definitions_walk.get()).into(),
            structure_defs: 0,
            aliases: 0,
        }
    }

    /// Writes the definition of projection `id`, split across events if it is big. Sizes are
    /// estimated, not exact.
    fn write_tset_def(&self, id: u64, key: &TransitiveSetProjectionKey, node: DirectProducers) {
        let mut event = buck2_data::TransitiveSetProjectionDef {
            tset_id: id,
            display_key: key.to_string().chars().take(DISPLAY_KEY_CHARS).collect(),
            ..Default::default()
        };
        let mut producers = buck2_data::ActionGraphProducers::default();
        // Encoded sizes, rounded up: `id` takes at most 11 bytes (a 1-byte tag and a varint of up
        // to 10) and `display_key` adds a tag and a 2-byte length. The producers message and its
        // three lists add a tag and a length of at most 4 bytes each, and each id in a list takes
        // at most 10.
        let mut size = event.display_key.len() + 34;
        let lists: [(
            &[u64],
            fn(&mut buck2_data::ActionGraphProducers) -> &mut Vec<u64>,
        ); 3] = [
            (&node.span_ids, |p| &mut p.span_ids),
            (&node.reused_ids, |p| &mut p.reused_ids),
            (&node.tset_ids, |p| &mut p.tset_ids),
        ];
        for (ids, list) in lists {
            for &item in ids {
                if size + 10 > TSET_DEF_BUDGET_BYTES {
                    event.producers = Some(std::mem::take(&mut producers));
                    self.emit_tset_def(std::mem::replace(
                        &mut event,
                        buck2_data::TransitiveSetProjectionDef {
                            tset_id: id,
                            ..Default::default()
                        },
                    ));
                    size = 31;
                }
                size += 10;
                list(&mut producers).push(item);
            }
        }
        event.producers = Some(producers);
        self.emit_tset_def(event);
    }

    fn emit_tset_def(&self, event: buck2_data::TransitiveSetProjectionDef) {
        instant_event(event);
        self.0.tset_def_events.fetch_add(1, Ordering::Relaxed);
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
    /// Inputs that arrive through a transitive set, by the projection's per-command id.
    tset_ids: Vec<u64>,
}

impl DirectProducers {
    /// Finds the producers of `inputs`. An input that arrives through a transitive set is
    /// recorded as the projection's id, not expanded.
    pub(crate) fn collect<'a>(
        recorder: &GraphRecorder,
        inputs: impl IntoIterator<Item = &'a ArtifactGroup>,
    ) -> Self {
        let start = Instant::now();
        let result = Self::collect_uncounted(recorder, inputs);
        recorder.0.capture.add(Instant::now() - start);
        result
    }

    /// Like `collect`, without adding the time to the footer's `capture_us`.
    fn collect_uncounted<'a>(
        recorder: &GraphRecorder,
        inputs: impl IntoIterator<Item = &'a ArtifactGroup>,
    ) -> Self {
        let (keys, tsets) = producer_keys(inputs);
        let mut result = DirectProducers::default();
        if !tsets.is_empty() {
            let mut ids = recorder.0.tset_ids.lock().expect("poisoned lock");
            result.tset_ids = tsets
                .into_iter()
                .map(|key| {
                    let next = ids.len() as u64;
                    *ids.entry(key.dupe()).or_insert(next)
                })
                .collect();
            result.tset_ids.sort_unstable();
            result.tset_ids.dedup();
        }
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
        result
    }

    pub(crate) fn into_proto(self) -> buck2_data::ActionGraphProducers {
        buck2_data::ActionGraphProducers {
            span_ids: self.span_ids,
            reused_ids: self.reused_ids,
            tset_ids: self.tset_ids,
        }
    }
}

/// The producers of `inputs` by action key, and the transitive-set projections among them.
fn producer_keys<'a>(
    inputs: impl IntoIterator<Item = &'a ArtifactGroup>,
) -> (
    BuckMutSet<&'a ActionKey>,
    Vec<&'a TransitiveSetProjectionKey>,
) {
    let mut keys = BuckMutSet::<&ActionKey>::default();
    let mut tsets = Vec::new();
    for input in inputs {
        match input {
            ArtifactGroup::Artifact(artifact) => keys.extend(artifact.action_key()),
            ArtifactGroup::Promise(promise) => keys.extend(
                promise
                    .promise_artifact
                    .get()
                    .and_then(|artifact| artifact.action_key()),
            ),
            ArtifactGroup::TransitiveSetProjection(tset) => tsets.push(&tset.key),
        }
    }
    (keys, tsets)
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
    work.then(async move |result| {
        if recording {
            let incomplete = match &result {
                Ok(value) => incomplete(value),
                Err(_) => Some(Incomplete::BuildFailed),
            };
            finish_recording(&mut ctx.ctx(), incomplete).await;
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

/// Ends a command that records the action graph: writes the transitive-set definitions the graph
/// refers to, then the footer.
async fn finish_recording(ctx: &mut DiceComputations<'_>, incomplete: Option<Incomplete>) {
    let recorder = ctx.per_transaction_data().get_graph_recorder().dupe();
    let finished = write_tset_definitions(ctx, &recorder).await;
    let incomplete = incomplete.or((!finished).then_some(Incomplete::UnfinishedWalk));
    instant_event(recorder.footer(incomplete));
}

/// Writes a definition for each transitive-set projection that actions referred to, and each
/// projection below those, in one walk after the command. Returns `false` if the walk could not
/// finish, in which case some referenced ids stay undefined.
async fn write_tset_definitions(ctx: &mut DiceComputations<'_>, recorder: &GraphRecorder) -> bool {
    let start = Instant::now();
    let mut referenced = recorder.referenced_tsets();
    referenced.sort_unstable_by_key(|(_, id)| *id);
    let mut pending: VecDeque<_> = referenced.into();
    let mut written = BuckMutSet::default();
    let mut finished = true;
    let mut steps: u64 = 0;

    while let Some((key, id)) = pending.pop_front() {
        // Cached lookups complete without suspending, so without this a large walk would hold
        // its executor thread for its whole length and delay cancellation.
        steps += 1;
        if steps.is_multiple_of(1000) {
            tokio::task::yield_now().await;
        }
        if !written.insert(id) {
            continue;
        }
        let sub_inputs = match key.key.lookup(ctx).await {
            Ok(set) => set.by_ref(|s| s.get_projection_sub_inputs(key.projection)),
            Err(e) => Err(e),
        };
        let sub_inputs = match sub_inputs {
            Ok(sub_inputs) => sub_inputs,
            Err(e) => {
                tracing::debug!("Could not define transitive set `{key}`: {e:#}");
                finished = false;
                continue;
            }
        };
        let node = DirectProducers::collect_uncounted(recorder, &sub_inputs);
        for input in &sub_inputs {
            if let ArtifactGroup::TransitiveSetProjection(child) = input {
                let child_id = recorder.tset_id(&child.key);
                if !written.contains(&child_id) {
                    pending.push_back((child.key.dupe(), child_id));
                }
            }
        }
        recorder.write_tset_def(id, &key, node);
    }

    recorder.0.definitions_walk.add(Instant::now() - start);
    finished
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
    use crate::artifact_groups::TransitiveSetProjectionWrapper;
    use crate::artifact_groups::deferred::TransitiveSetIndex;
    use crate::artifact_groups::deferred::TransitiveSetKey;

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

    fn tset(index: u32) -> ArtifactGroup {
        let ArtifactGroup::Artifact(artifact) = build_artifact(0) else {
            unreachable!()
        };
        let holder = artifact.action_key().unwrap().holder_key().dupe();
        ArtifactGroup::TransitiveSetProjection(Arc::new(TransitiveSetProjectionWrapper {
            key: TransitiveSetProjectionKey {
                key: TransitiveSetKey::new(holder, TransitiveSetIndex(index)),
                projection: 0,
            },
            path_resolution_may_require_artifact_value: false,
            is_eligible_for_dedupe: false,
        }))
    }

    #[test]
    fn test_tsets_get_stable_per_command_ids() {
        let recorder = GraphRecorder::default();
        let (a, b) = (tset(1), tset(2));
        let first = DirectProducers::collect(&recorder, [&a, &b, &a]);
        assert_eq!(
            first.tset_ids,
            vec![0, 1],
            "numbered in order of first reference, once each"
        );
        let second = DirectProducers::collect(&recorder, [&b]);
        assert_eq!(second.tset_ids, vec![1], "the same projection keeps its id");
        assert_eq!(recorder.0.tset_ids.lock().expect("poisoned lock").len(), 2);
    }

    #[test]
    fn test_big_tset_definitions_are_split() {
        let recorder = GraphRecorder::default();
        let ArtifactGroup::TransitiveSetProjection(t) = tset(1) else {
            unreachable!()
        };
        let node = DirectProducers {
            span_ids: (0..TSET_DEF_BUDGET_BYTES as u64 / 10 + 1).collect(),
            ..Default::default()
        };
        buck2_events::dispatch::with_dispatcher(
            buck2_events::dispatch::EventDispatcher::null(),
            || recorder.write_tset_def(0, &t.key, node),
        );
        assert_eq!(
            recorder.footer(None).tset_def_events,
            2,
            "one node over the budget, two events"
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
