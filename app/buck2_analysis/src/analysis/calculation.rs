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

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::analysis::AnalysisKeyActivationData;
use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::analysis::AnalysisWithExtraData;
use buck2_build_api::analysis::AnonTargetSplitData;
use buck2_build_api::analysis::calculation::EVAL_ANALYSIS_QUERY;
use buck2_build_api::analysis::calculation::RULE_ANALYSIS_CALCULATION;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculationImpl;
use buck2_build_api::build::detailed_aggregated_metrics::dice::HasDetailedAggregatedMetrics;
use buck2_build_api::deferred::calculation::DeferredHolder;
use buck2_build_api::keep_going::KeepGoing;
use buck2_build_signals::env::WaitingData;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::configuration::compatibility::ResultMaybeCompatibleValueSerialize;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_data::ToProtoMessage;
use buck2_data::error::ErrorTag;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_events::dispatch::async_record_root_spans;
use buck2_events::dispatch::record_root_spans;
use buck2_events::dispatch::span_async;
use buck2_events::dispatch::span_async_simple;
use buck2_hash::BuckMutMap;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_interpreter::file_loader::LoadedModule;
use buck2_interpreter::load_module::InterpreterCalculation;
use buck2_interpreter::paths::module::StarlarkModulePath;
use buck2_interpreter::starlark_profiler::config::GetStarlarkProfilerInstrumentation;
use buck2_interpreter::starlark_profiler::data::StarlarkProfileDataAndStats;
use buck2_interpreter::starlark_profiler::mode::StarlarkProfileMode;
use buck2_node::attrs::attr_type::query::ResolvedQueryLiterals;
use buck2_node::bzl_or_bxl_path::BzlOrBxlPath;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured::ConfiguredTargetNodeRef;
use buck2_node::nodes::configured_frontend::ConfiguredTargetNodeCalculation;
use buck2_node::rule_type::RuleType;
use buck2_node::rule_type::StarlarkRuleType;
use buck2_query::query::syntax::simple::eval::label_indexed::LabelIndexedSet;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_util::time_span::TimeSpan;
use dice::CancellationContext;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::ValueSerialize;
use dupe::Dupe;
use dupe::IterDupedExt;
use futures::FutureExt;
use futures::future::BoxFuture;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::analysis::env::RuleSpec;
use crate::analysis::env::get_user_defined_rule_spec;
use crate::analysis::env::run_analysis;
use crate::attrs::resolve::ctx::AnalysisQueryResult;

struct RuleAnalysisCalculationInstance;

#[derive(
    Clone,
    Dupe,
    derive_more::Display,
    Debug,
    Eq,
    Hash,
    PartialEq,
    Allocative,
    Pagable
)]
#[display("{}", _0)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub struct AnalysisKey(pub ConfiguredTargetLabel);

pub(crate) fn init_rule_analysis_calculation() {
    RULE_ANALYSIS_CALCULATION.init(&RuleAnalysisCalculationInstance);
}

#[async_trait]
impl Key for AnalysisKey {
    type Value = ResultMaybeCompatible<AnalysisResult>;
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellation: &CancellationContext,
    ) -> Self::Value {
        let deferred_key = DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(self.0.dupe()));
        ctx.analysis_started(&deferred_key)?;
        let res = get_analysis_result(ctx, &self.0, cancellation)
            .await
            .with_buck_error_context(|| format!("Error running analysis for `{}`", self.0))?;
        if let MaybeCompatible::Compatible(v) = &res {
            ctx.analysis_complete(&deferred_key, &DeferredHolder::Analysis(v))?;
        }
        res.to_result_maybe_compatible()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        // TODO consider if we want analysis result to be eq
        EqualityBehavior::AlwaysUnequal
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        ResultMaybeCompatibleValueSerialize::<AnalysisResult>::new()
    }
}

impl RuleAnalysisCalculationImpl for RuleAnalysisCalculationInstance {
    fn get_analysis_result<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        target: &'a ConfiguredTargetLabel,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d AnalysisResult>>
    where
        'd: 'a,
    {
        async move { ctx.compute(&AnalysisKey(target.dupe())).await?.as_ref() }.boxed()
    }
}

pub async fn resolve_queries(
    ctx: &mut DiceComputations<'_>,
    configured_node: ConfiguredTargetNodeRef<'_>,
) -> buck2_error::Result<BuckMutMap<String, Arc<AnalysisQueryResult>>> {
    let mut queries = configured_node.body()?.queries().peekable();

    if queries.peek().is_none() {
        return Ok(Default::default());
    }

    span_async_simple(
        buck2_data::AnalysisResolveQueriesStart {
            standard_target: Some(configured_node.label().as_proto()),
        },
        resolve_queries_impl(ctx, configured_node, queries),
        buck2_data::AnalysisResolveQueriesEnd {},
    )
    .await
}

async fn resolve_queries_impl(
    ctx: &mut DiceComputations<'_>,
    configured_node: ConfiguredTargetNodeRef<'_>,
    queries: impl IntoIterator<Item = (String, ResolvedQueryLiterals<ConfiguredProvidersLabel>)>,
) -> buck2_error::Result<BuckMutMap<String, Arc<AnalysisQueryResult>>> {
    let deps: TargetSet<_> = configured_node.deps().iter().duped().collect();
    let queries: Vec<_> = queries.into_iter().collect();
    let query_results = ctx
        .try_compute_join(
            queries,
            async |ctx,
                   (query, resolved_literals_labels): (
                String,
                ResolvedQueryLiterals<ConfiguredProvidersLabel>,
            )| {
                let mut resolved_literals = BuckMutMap::with_capacity_and_hasher(
                    resolved_literals_labels.0.len(),
                    Default::default(),
                );
                for ((offset, len), label) in resolved_literals_labels.0 {
                    let literal = &query[offset..offset + len];
                    let node = deps.get(label.target()).ok_or_else(|| {
                        internal_error!("Literal `{literal}` not found in `deps`")
                    })?;
                    resolved_literals.insert(literal.to_owned(), node.dupe());
                }

                let result = (EVAL_ANALYSIS_QUERY.get()?)(ctx, &query, resolved_literals).await?;

                // analysis for all the deps in the query result should already have been run since they must
                // be in our dependency graph, and so we don't worry about parallelizing these lookups.
                let mut query_results = Vec::new();
                for node in result.iter() {
                    let label = node.label();
                    query_results.push((
                        label.dupe(),
                        ctx.get_analysis_result(label)
                            .await
                            .require_compatible()?
                            .providers()?
                            .to_owned(),
                    ))
                }

                buck2_error::Ok((
                    query.to_owned(),
                    Arc::new(AnalysisQueryResult {
                        result: query_results,
                    }),
                ))
            },
        )
        .await?;

    let query_results: BuckMutMap<_, _> = query_results.into_iter().collect();
    Ok(query_results)
}

pub async fn get_dep_analysis<'v>(
    configured_node: ConfiguredTargetNodeRef<'v>,
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<Vec<(&'v ConfiguredTargetLabel, AnalysisResult)>> {
    KeepGoing::try_compute_join_all(ctx, configured_node.deps(), async |ctx, dep| {
        let res = ctx
            .get_analysis_result(dep.label())
            .await
            .require_compatible();
        res.map(|x| (dep.label(), x.dupe()))
    })
    .await
}

pub async fn get_loaded_module<'d>(
    ctx: &mut DiceComputations<'d>,
    func: &StarlarkRuleType,
) -> buck2_error::Result<&'d LoadedModule> {
    match &func.path {
        BzlOrBxlPath::Bxl(bxl_file_path) => {
            let module_path = StarlarkModulePath::BxlFile(bxl_file_path);
            ctx.get_loaded_module(module_path).await
        }
        BzlOrBxlPath::Bzl(import_path) => ctx.get_loaded_module_from_import_path(import_path).await,
    }
}

pub async fn get_rule_spec(
    ctx: &mut DiceComputations<'_>,
    func: &StarlarkRuleType,
) -> buck2_error::Result<impl RuleSpec + use<>> {
    let module = get_loaded_module(ctx, func).await?;
    Ok(get_user_defined_rule_spec(module.env().dupe(), func))
}

async fn get_analysis_result(
    ctx: &mut DiceComputations<'_>,
    target: &ConfiguredTargetLabel,
    cancellation: &CancellationContext,
) -> buck2_error::Result<MaybeCompatible<AnalysisResult>> {
    get_analysis_result_inner(ctx, target, cancellation)
        .await
        .tag(ErrorTag::Analysis)
}

async fn get_analysis_result_inner(
    ctx: &mut DiceComputations<'_>,
    target: &ConfiguredTargetLabel,
    cancellation: &CancellationContext,
) -> buck2_error::Result<MaybeCompatible<AnalysisResult>> {
    let configured_node: MaybeCompatible<&ConfiguredTargetNode> =
        ctx.get_configured_target_node(target).await.ok()?;
    let configured_node: &ConfiguredTargetNode = match configured_node {
        MaybeCompatible::Incompatible(reason) => {
            return Ok(MaybeCompatible::Incompatible(reason));
        }
        MaybeCompatible::Compatible(configured_node) => configured_node,
    };

    // For precision, grab the *actual* rule type and not the *underlying* rule type.
    let target_rule_type_name = configured_node.rule_type().name().to_owned();

    let configured_node = configured_node.as_ref();

    let ((res, now, split_instants), spans): ((buck2_error::Result<_>, _, _), _) =
        match configured_node.rule_type() {
            RuleType::Starlark(func) => {
                let (dep_analysis, query_results) = ctx
                    .try_compute2(
                        async |ctx| get_dep_analysis(configured_node, ctx).await,
                        async |ctx| resolve_queries(ctx, configured_node).await,
                    )
                    .await?;

                let now = std::time::Instant::now();
                let (res, spans) = async_record_root_spans(async {
                    let rule_spec = get_rule_spec(ctx, func).await?;
                    let start_event = buck2_data::AnalysisStart {
                        target: Some(target.as_proto().into()),
                        rule: func.to_string(),
                    };

                    let (result, split_instants) = span_async(start_event, async {
                        let mut profile = None;
                        let mut declared_artifacts = None;
                        let mut declared_actions = None;
                        let mut split_instants = None;

                        let result: buck2_error::Result<_> = try {
                            let (result, split) = span_async_simple(
                                buck2_data::AnalysisStageStart {
                                    stage: Some(
                                        buck2_data::analysis_stage_start::Stage::EvaluateRule(()),
                                    ),
                                },
                                run_analysis(
                                    ctx,
                                    target,
                                    dep_analysis,
                                    query_results,
                                    configured_node.body()?.execution_platform_resolution(),
                                    &rule_spec,
                                    configured_node,
                                    cancellation,
                                ),
                                buck2_data::AnalysisStageEnd {},
                            )
                            .await?;

                            split_instants = split;
                            profile = Some(make_analysis_profile(&result)?);
                            declared_artifacts = Some(result.num_declared_artifacts);
                            declared_actions = Some(result.num_declared_actions);

                            MaybeCompatible::Compatible(result)
                        };

                        let error = result.as_ref().err().map(|e| format!("{e:#}"));

                        (
                            (result, split_instants),
                            buck2_data::AnalysisEnd {
                                target: Some(target.as_proto().into()),
                                rule: func.to_string(),
                                profile,
                                declared_actions,
                                declared_artifacts,
                                error,
                            },
                        )
                    })
                    .await;

                    buck2_error::Ok((result, split_instants))
                })
                .await;

                match res {
                    Ok((result, split)) => ((result, now, split), spans),
                    Err(e) => ((Err(e), now, None), spans),
                }
            }
            RuleType::Forward => {
                let mut dep_analysis = get_dep_analysis(configured_node, ctx).await?;
                let now = std::time::Instant::now();
                let (res, spans) = record_root_spans(|| {
                    let one_dep_analysis = dep_analysis.pop().ok_or_else(|| {
                        internal_error!("Forward node analysis produced no results")
                    })?;
                    if !dep_analysis.is_empty() {
                        return Err(internal_error!(
                            "Forward node analysis produced more than one result"
                        ));
                    }
                    Ok(MaybeCompatible::Compatible(one_dep_analysis.1))
                });

                ((res, now, None), spans)
            }
        };

    let analysis_end = std::time::Instant::now();
    let (time_span, anon_target_split) = match split_instants {
        Some(split) => (
            TimeSpan::new_saturating(now, split.pre_promises),
            Some(AnonTargetSplitData {
                part2_time_span: TimeSpan::new_saturating(split.post_promises, analysis_end),
            }),
        ),
        None => (TimeSpan::new_saturating(now, analysis_end), None),
    };

    ctx.store_evaluation_data(AnalysisKeyActivationData {
        waiting_data: WaitingData::new(),
        time_span,
        spans,
        analysis_with_extra_data: AnalysisWithExtraData {
            target_rule_type_name: Some(target_rule_type_name),
        },
        anon_target_split,
    })?;

    res
}

fn make_analysis_profile(res: &AnalysisResult) -> buck2_error::Result<buck2_data::AnalysisProfile> {
    let heap = res.providers()?.owner();

    Ok(buck2_data::AnalysisProfile {
        starlark_allocated_bytes: heap.allocated_bytes() as u64,
        starlark_available_bytes: heap.available_bytes() as u64,
    })
}

fn all_deps(nodes: &[ConfiguredTargetNode]) -> LabelIndexedSet<ConfiguredTargetNode> {
    let mut stack = nodes.to_vec();
    let mut visited = LabelIndexedSet::new();
    let mut result = LabelIndexedSet::new();
    while let Some(node) = stack.pop() {
        if visited.insert(node.dupe()) {
            match node.rule_type() {
                RuleType::Starlark(_) => {
                    result.insert(node.dupe());
                }
                RuleType::Forward => {
                    // No starlark code ran on forward node.
                }
            }

            stack.extend(node.deps().iter().duped());
        }
    }
    result
}

pub async fn profile_analysis(
    ctx: &mut DiceComputations<'_>,
    targets: &[ConfiguredTargetLabel],
) -> buck2_error::Result<StarlarkProfileDataAndStats> {
    // Self check.
    for target in targets {
        let profile_mode = ctx
            .get_starlark_profiler_mode(&StarlarkEvalKind::Analysis(target.dupe()))
            .await?;
        if !matches!(profile_mode, StarlarkProfileMode::Profile(_)) {
            return Err(internal_error!("recursive analysis configured incorrectly"));
        }
    }

    let nodes: Vec<ConfiguredTargetNode> = ctx
        .try_compute_join(targets.iter(), async |ctx, target| {
            let node = ctx
                .get_configured_target_node(target)
                .await
                .require_compatible()?;
            buck2_error::Ok(node.dupe())
        })
        .await?;

    let all_deps = all_deps(&nodes);

    let profile_datas = ctx
        .try_compute_join(all_deps.iter(), async |ctx, node| {
            let result = ctx
                .get_analysis_result(node.label())
                .await
                .require_compatible()?;
            // This may be `None` if we are running profiling for a subset of the targets.
            buck2_error::Ok(result.profile_data.dupe())
        })
        .await?;

    StarlarkProfileDataAndStats::merge(
        profile_datas
            .iter()
            .filter_map(|o| o.as_ref())
            .map(|x| &**x),
    )
}
