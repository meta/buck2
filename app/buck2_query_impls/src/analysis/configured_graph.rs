/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::VecDeque;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_node::nodes::configured_ref::ConfiguredGraphNodeRef;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use derive_more::Display;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::LinearRecomputeDiceComputations;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::IterDupedExt;
use dupe::OptionDupedExt;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::StaticStr;
use pagable::pagable_typetag;

use crate::analysis::environment::ConfiguredGraphQueryEnvironmentDelegate;
use crate::analysis::environment::get_from_template_placeholder_info;

pub(crate) struct AnalysisDiceQueryDelegate<'c, 'd> {
    pub(crate) ctx: LinearRecomputeDiceComputations<'c, 'd>,
}

impl AnalysisDiceQueryDelegate<'_, '_> {
    pub(crate) fn ctx(&self) -> DiceComputations<'_> {
        self.ctx.get()
    }
}

pub(crate) struct AnalysisConfiguredGraphQueryDelegate<'a, 'd> {
    pub(crate) dice_query_delegate: Arc<AnalysisDiceQueryDelegate<'a, 'd>>,
    pub(crate) resolved_literals: BuckMutMap<String, ConfiguredTargetNode>,
}

#[async_trait]
impl ConfiguredGraphQueryEnvironmentDelegate for AnalysisConfiguredGraphQueryDelegate<'_, '_> {
    fn eval_literal(&self, literal: &str) -> buck2_error::Result<ConfiguredTargetNode> {
        self.resolved_literals
            .get(literal)
            .duped()
            .ok_or_else(|| buck2_error::buck2_error!(buck2_error::ErrorTag::Tier0, ""))
    }

    async fn get_targets_from_template_placeholder_info(
        &self,
        template_name: StaticStr,
        targets: TargetSet<ConfiguredGraphNodeRef>,
    ) -> buck2_error::Result<TargetSet<ConfiguredGraphNodeRef>> {
        #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
        #[display("template_placeholder_info_query({})", template_name)]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct TemplatePlaceholderInfoQueryKey {
            template_name: StaticStr,
            // Use `ConfiguredTargetLabel` instead of `ConfiguredGraphNodeRef` here because `ConfiguredGraphNodeRef`
            // only computes Hash and PartialEq based on the label. If we use `ConfiguredGraphNodeRef` directly we
            // may cache stale ConfiguredTargetNodes and end up with a bug like T133069783.
            targets: Arc<Vec<ConfiguredTargetLabel>>,
        }

        #[async_trait]
        impl Key for TemplatePlaceholderInfoQueryKey {
            type Value = buck2_error::Result<TargetSet<ConfiguredGraphNodeRef>>;

            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellation: &CancellationContext,
            ) -> Self::Value {
                let (targets, label_to_artifact) = {
                    ctx.try_compute2(
                        async |ctx| {
                            ctx.try_compute_join(self.targets.iter(), async |ctx, target| {
                                ctx.get_configured_target_node(target)
                                    .await
                                    .require_compatible()
                                    .map(|n| n.dupe())
                            })
                            .await
                        },
                        async |ctx| {
                            get_from_template_placeholder_info(
                                ctx,
                                self.template_name,
                                self.targets.iter().duped(),
                            )
                            .await
                        },
                    )
                    .await?
                };

                let targets: TargetSet<_> = targets
                    .into_iter()
                    .map(ConfiguredGraphNodeRef::new)
                    .collect();
                let targets = find_target_nodes(targets, label_to_artifact)?;
                Ok(targets)
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|_, _| {
                    // result is not comparable
                    false
                })
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        let targets: Vec<_> = targets
            .into_iter()
            .map(|target| target.label().dupe())
            .collect();
        Ok(self
            .dice_query_delegate
            .ctx()
            .compute(&TemplatePlaceholderInfoQueryKey {
                template_name,
                targets: Arc::new(targets),
            })
            .await?
            .as_ref()
            .duped_err()?
            .clone())
    }
}

/// Finds the nodes for a list of target labels within the deps of the provided targets.
///
/// It may seem like if we have ConfiguredTargetLabel we should just be able to lookup the
/// nodes directly, but that would require going through dice and then dice would record
/// dependencies on all the nodes that we lookup. It's common for these queries to operate
/// over inputs that are aggregated as data flows up the graph and it's important that we
/// don't inadvertently cause flattening of those sets.
fn find_target_nodes(
    targets: TargetSet<ConfiguredGraphNodeRef>,
    label_to_artifact: BuckMutInsertionMap<ConfiguredTargetLabel, Artifact>,
) -> buck2_error::Result<TargetSet<ConfiguredGraphNodeRef>> {
    let mut queue: VecDeque<_> = targets.iter().duped().collect();
    let mut seen = targets;
    let mut result = TargetSet::new();

    while let Some(target) = queue.pop_front() {
        if label_to_artifact.contains_key(target.label()) {
            result.insert(target.dupe());
        }
        if result.len() == label_to_artifact.len() {
            return Ok(result);
        }
        for dep in target.target_deps() {
            let dep = ConfiguredGraphNodeRef::new(dep.dupe());
            if seen.insert(dep.dupe()) {
                queue.push_back(dep);
            }
        }
    }

    Ok(result)
}
