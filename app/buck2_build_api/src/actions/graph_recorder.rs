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

use std::sync::Arc;
use std::sync::Mutex;

use buck2_artifact::actions::key::ActionKey;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use dice::UserComputationData;
use dupe::Dupe;

use crate::artifact_groups::ArtifactGroup;

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
}

impl GraphRecorder {
    pub(crate) fn record(&self, action: &ActionKey, span_id: u64) {
        self.0
            .producer_spans
            .lock()
            .expect("poisoned lock")
            .insert(action.dupe(), span_id);
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
