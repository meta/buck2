/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use buck2_artifact::actions::key::ActionIndex;
    use buck2_artifact::actions::key::ActionKey;
    use buck2_artifact::artifact::build_artifact::BuildArtifact;
    use buck2_artifact::artifact::source_artifact::SourceArtifact;
    use buck2_build_api::actions::RegisteredAction;
    use buck2_build_api::actions::registry::RecordedActions;
    use buck2_build_api::analysis::AnalysisResult;
    use buck2_build_api::analysis::registry::RecordedAnalysisValues;
    use buck2_build_api::artifact_groups::ArtifactGroup;
    use buck2_build_api::artifact_groups::deferred::TransitiveSetKey;
    use buck2_build_api::build::detailed_aggregated_metrics::testing::traverse_partial_action_graph;
    use buck2_build_api::deferred::calculation::OwnedDeferredHolder;
    use buck2_build_api::dynamic::calculation::DynamicLambdaResult;
    use buck2_build_api::interpreter::rule_defs::transitive_set::TransitiveSet;
    use buck2_core::category::Category;
    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
    use buck2_core::deferred::dynamic::DynamicLambdaIndex;
    use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
    use buck2_core::deferred::key::DeferredHolderKey;
    use buck2_core::execution_types::executor_config::CommandExecutorConfig;
    use buck2_core::fs::buck_out_path::BuckOutPathKind;
    use buck2_core::fs::buck_out_path::BuildArtifactPath;
    use buck2_core::package::source_path::SourcePath;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    use buck2_hash::BuckFixedSortedMap;
    use buck2_hash::buck_mut_insertion_set;
    use buck2_node::rule_type::RuleType;
    use dupe::Dupe;
    use dupe::IterDupedExt;
    use starlark::values::OwnedFrozen;
    use starlark::values::ValueTyped;

    use crate::actions::testings::SimpleAction;

    enum ActionState {
        UnboundDynamic,
        Action(Arc<RegisteredAction>),
        BoundDynamic(ActionKey),
    }

    struct Builder {
        holder_key: DeferredHolderKey,
        action_count: u32,
        actions: Vec<(ActionKey, ActionState)>,
        tsets: Vec<(
            TransitiveSetKey,
            OwnedFrozen<ValueTyped<'static, TransitiveSet<'static>>>,
        )>,
        dynamics: Vec<()>,
    }

    impl Builder {
        fn for_analysis(target: ConfiguredTargetLabel) -> Self {
            Self::new(DeferredHolderKey::for_analysis(target))
        }

        fn for_dynamic(key: DynamicLambdaResultsKey) -> Self {
            Self::new(DeferredHolderKey::for_dynamic_lambda(key))
        }

        fn new(holder_key: DeferredHolderKey) -> Self {
            Self {
                holder_key,
                action_count: 0,
                actions: Vec::new(),
                tsets: Vec::new(),
                dynamics: Vec::new(),
            }
        }

        fn add_to_state(
            self,
            state: &mut buck2_hash::BuckMutMap<DeferredHolderKey, OwnedDeferredHolder>,
        ) {
            let (key, holder) = self.build();
            state.insert(key, holder);
        }

        fn build(self) -> (DeferredHolderKey, OwnedDeferredHolder) {
            let Self {
                holder_key,
                actions,
                tsets,
                ..
            } = self;
            let mut recorded_actions = RecordedActions::new(actions.len());
            for (key, action) in actions {
                match action {
                    ActionState::UnboundDynamic => panic!("unbound dynamic"),
                    ActionState::Action(action) => recorded_actions.insert(key, action),
                    ActionState::BoundDynamic(action_key) => {
                        recorded_actions.insert_dynamic_output(key, action_key)
                    }
                }
            }
            let analysis_values =
                RecordedAnalysisValues::testing_new(holder_key.dupe(), tsets, recorded_actions);
            let holder = match &holder_key {
                DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(..)) => {
                    OwnedDeferredHolder::Analysis(
                        AnalysisResult::new(
                            analysis_values,
                            None,
                            BuckFixedSortedMap::default(),
                            0,
                            0,
                            None,
                            Vec::new(),
                            RuleType::Forward,
                        )
                        .expect("test analysis should have valid providers"),
                    )
                }
                DeferredHolderKey::DynamicLambda(..) => {
                    OwnedDeferredHolder::DynamicLambda(Arc::new(DynamicLambdaResult {
                        analysis_values,
                        rule_type: None,
                    }))
                }
                _ => unimplemented!(),
            };

            (holder_key, holder)
        }

        fn add_dynamic(&mut self, _dynamic_inputs: Vec<ArtifactGroup>) -> DynamicLambdaResultsKey {
            let key = DynamicLambdaResultsKey::new(
                self.holder_key.dupe(),
                DynamicLambdaIndex::new(self.dynamics.len() as u32),
            );
            self.dynamics.push(());
            key
        }

        fn bind_dynamic(&mut self, output_slot: ArtifactGroup, bind_to: &ActionKey) {
            let ArtifactGroup::Artifact(artifact) = output_slot else {
                panic!("expected an artifact")
            };
            let key = artifact.action_key().unwrap();
            match &mut self.actions[key.action_index().0 as usize].1 {
                ActionState::Action(..) | ActionState::BoundDynamic(..) => panic!(),
                x => *x = ActionState::BoundDynamic(bind_to.dupe()),
            }
        }

        fn allocate_action(&mut self) -> ActionKey {
            assert!(self.actions.len() == self.action_count as usize);
            let key = ActionKey::new(self.holder_key.dupe(), ActionIndex::new(self.action_count));
            self.action_count += 1;
            key
        }

        fn allocate_dynamic_output_slot(&mut self) -> ArtifactGroup {
            assert!(self.holder_key.is_dynamic());
            assert!(
                self.actions
                    .last()
                    .is_none_or(|(_, state)| matches!(state, ActionState::UnboundDynamic))
            );
            let key = self.allocate_action();
            self.actions.push((key.dupe(), ActionState::UnboundDynamic));
            ArtifactGroup::Artifact(self.artifact_for_key(&key).into())
        }

        fn add_action(&mut self, inputs: Vec<&ArtifactGroup>) -> (ArtifactGroup, ActionKey) {
            let key = self.allocate_action();
            let idx = key.action_index().0;
            let output = self.artifact_for_key(&key);
            let action = RegisteredAction::new(
                key.dupe(),
                Box::new(SimpleAction::new(
                    inputs.into_iter().duped().collect(),
                    buck_mut_insertion_set! {output.dupe()},
                    Vec::new(),
                    Category::new("category".to_owned()).unwrap(),
                    Some(format!("id-{idx}")),
                )),
                CommandExecutorConfig::testing_local(),
            );
            self.actions
                .push((key.dupe(), ActionState::Action(Arc::new(action))));
            (ArtifactGroup::Artifact(output.into()), key)
        }

        fn artifact_for_key(&self, key: &ActionKey) -> BuildArtifact {
            BuildArtifact::new(
                BuildArtifactPath::new(
                    self.holder_key.owner().dupe(),
                    ForwardRelativePathBuf::unchecked_new(format!("output-{}", key.action_index())),
                    BuckOutPathKind::default(),
                ),
                key.dupe(),
                buck2_artifact::artifact::output_type::OutputType::File,
            )
            .unwrap()
        }

        // Can be used to get an artifact for an action before you've added the action.
        fn artifact_for_action_idx(&self, arg: i32) -> ArtifactGroup {
            ArtifactGroup::Artifact(
                self.artifact_for_key(&ActionKey::new(
                    self.holder_key.dupe(),
                    ActionIndex::new(arg as u32),
                ))
                .into(),
            )
        }
    }

    fn create_target(name: &str) -> ConfiguredTargetLabel {
        let cfg = ConfigurationData::testing_new();
        ConfiguredTargetLabel::testing_parse(name, cfg)
    }

    fn create_source_artifact(pkg: &str, path: &str) -> ArtifactGroup {
        let source = SourceArtifact::new(SourcePath::testing_new(pkg, path));
        ArtifactGroup::Artifact(source.into())
    }

    #[track_caller]
    fn assert_set_eq(actions: buck2_hash::BuckMutSet<ActionKey>, expected: Vec<ActionKey>) {
        let expected_set: buck2_hash::BuckMutSet<_> = expected.into_iter().collect();
        assert_eq!(actions, expected_set)
    }

    #[test]
    fn test_empty_graph() -> buck2_error::Result<()> {
        let state = buck2_hash::BuckMutMap::default();
        let (complete, actions) = traverse_partial_action_graph(Vec::new(), &state)?;
        assert!(complete);
        assert!(actions.is_empty());
        Ok(())
    }

    #[test]
    fn test_single_action() -> buck2_error::Result<()> {
        let mut state = buck2_hash::BuckMutMap::default();

        let mut builder = Builder::for_analysis(create_target("root//:lib").dupe());
        let (output, action_key) =
            builder.add_action(vec![&create_source_artifact("root//", "src/main.rs")]);
        builder.add_to_state(&mut state);

        let (complete, actions) = traverse_partial_action_graph([&output], &state)?;

        assert!(complete);
        assert_set_eq(actions, vec![action_key]);
        Ok(())
    }

    #[test]
    fn test_diamond() -> buck2_error::Result<()> {
        let mut state = buck2_hash::BuckMutMap::default();

        let mut builder1 = Builder::for_analysis(create_target("root//:lib").dupe());
        let mut builder2 = Builder::for_analysis(create_target("root//:bin"));

        let (output1, action_key1) =
            builder1.add_action(vec![&create_source_artifact("root//", "src/main.rs")]);
        let (output2, action_key2) = builder1.add_action(vec![&output1]);
        let (output3, action_key3) = builder1.add_action(vec![&output1]);

        let (output4, action_key4) = builder2.add_action(vec![&output2, &output3]);

        // Add some unreferenced actions;
        builder1.add_action(Vec::new());
        builder1.add_action(Vec::new());
        builder2.add_action(Vec::new());

        builder1.add_to_state(&mut state);
        builder2.add_to_state(&mut state);

        let (complete, actions) = traverse_partial_action_graph([&output4], &state)?;

        assert!(complete);
        assert_set_eq(
            actions,
            vec![action_key1, action_key2, action_key3, action_key4],
        );
        Ok(())
    }

    #[test]
    fn test_cycle() -> buck2_error::Result<()> {
        // cycles can't actually occur in the action graph, but there may be bugs or races in our
        // state tracking that leads to us traversing an invalid graph. This test just ensures that
        // we still terminate if we encounter a cycle.
        let mut state = buck2_hash::BuckMutMap::default();

        let mut builder1 = Builder::for_analysis(create_target("root//:lib").dupe());

        let output2 = builder1.artifact_for_action_idx(1);
        let (output1, action_key1) = builder1.add_action(vec![&output2]);
        let (output2_2, action_key2) = builder1.add_action(vec![&output1]);

        assert_eq!(output2, output2_2);

        builder1.add_to_state(&mut state);

        let (complete, actions) = traverse_partial_action_graph([&output2], &state)?;

        assert!(complete);
        assert_set_eq(actions, vec![action_key1, action_key2]);
        Ok(())
    }

    /// Checks that we include edge to the inputs of a dynamic node.
    #[test]
    #[should_panic(expected = "assertion `left == right` failed")] // We don't currently actually have the ability to traverse this edge.
    fn test_dynamic_input() {
        fn go() -> buck2_error::Result<()> {
            let mut state = buck2_hash::BuckMutMap::default();

            let mut builder1 = Builder::for_analysis(create_target("root//:lib").dupe());

            let (output1, action_key1) = builder1.add_action(vec![]);

            let dynamic_key1 = builder1.add_dynamic(vec![output1]);
            let mut dynamic_builder1 = Builder::for_dynamic(dynamic_key1);
            let dynamic_output1 = dynamic_builder1.allocate_dynamic_output_slot();
            let (final_output, final_action_key) = builder1.add_action(vec![&dynamic_output1]);

            let (_, action_key2) = dynamic_builder1.add_action(vec![]);
            dynamic_builder1.bind_dynamic(dynamic_output1.dupe(), &action_key2);

            builder1.add_to_state(&mut state);
            dynamic_builder1.add_to_state(&mut state);

            let (complete, actions) = traverse_partial_action_graph([&final_output], &state)?;

            assert!(complete);
            assert_set_eq(actions, vec![final_action_key, action_key2, action_key1]);
            Ok(())
        }
        go().unwrap();
    }

    /// Checks that we can traverse a graph where some parts are missing.
    #[test]
    fn test_dynamic_node_analysis_missing() -> buck2_error::Result<()> {
        let mut state = buck2_hash::BuckMutMap::default();

        let mut builder1 = Builder::for_analysis(create_target("root//:lib").dupe());

        let dynamic_key1 = builder1.add_dynamic(vec![]);
        let mut dynamic_builder1 = Builder::for_dynamic(dynamic_key1);
        let dynamic_output1 = dynamic_builder1.allocate_dynamic_output_slot();

        let (output1, action_key1) = builder1.add_action(vec![]);

        let (final_output, final_action_key) =
            builder1.add_action(vec![&dynamic_output1, &output1]);

        builder1.add_to_state(&mut state);
        // dynamic_builder1 is not added to the state.

        let (complete, actions) = traverse_partial_action_graph([&final_output], &state)?;

        assert!(!complete);
        assert_set_eq(actions, vec![final_action_key, action_key1]);
        Ok(())
    }

    // TODO(cjhopman): Implement remaining stuff for this test. Creating tsets for tests is extremely difficult currently.
    /*
    #[test]
    fn test_tset() -> buck2_error::Result<()> {
        let mut state = buck2_hash::BuckMutMap::default();

        let mut builder1 = Builder::for_analysis(create_target("root//:lib").dupe());

        let (output1, action_key1) = builder1.add_action(vec![]);
        let (output2, action_key2) = builder1.add_action(vec![]);
        let (output3, action_key3) = builder1.add_action(vec![]);

        let tset1 = builder1.add_tset(output1, vec![]);
        let tset2 = builder1.add_tset(output2, vec![&tset1]);
        let tset3 = builder1.add_tset(output3, vec![&tset2]);

        let proj = (|| todo!())();

        let (final_output, final_action_key) = builder1.add_action(vec![&proj]);
        builder1.add_to_state(&mut state);

        let (complete, actions) = traverse_partial_action_graph([&final_output], &state)?;

        assert!(complete);
        assert_set_eq(actions, vec![final_action_key, action_key1, action_key2, action_key3]);
        Ok(())
    }
    */
}
