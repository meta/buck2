/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::iter;
use std::sync::Arc;

use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::analysis::AnalysisKeyActivationData;
use buck2_build_api::analysis::AnalysisWithExtraData;
use buck2_build_api::analysis::registry::AnalysisRegistry;
use buck2_build_api::analysis::registry::RecordedAnalysisValues;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::artifact_groups::ArtifactGroupValues;
use buck2_build_api::artifact_groups::calculation::ArtifactGroupCalculation;
use buck2_build_api::dynamic::calculation::dynamic_lambda_result;
use buck2_build_api::dynamic_value::DynamicValue;
use buck2_build_api::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_value::StarlarkArtifactValue;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::FrozenStarlarkOutputArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifactView;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_build_api::interpreter::rule_defs::context::AnalysisContext;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_build_api::materialize::invocation_re_use_case;
use buck2_build_signals::env::WaitingCategory;
use buck2_build_signals::env::WaitingData;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_events::dispatch::get_dispatcher;
use buck2_events::dispatch::record_root_spans;
use buck2_events::dispatch::span;
use buck2_events::dispatch::span_async_simple;
use buck2_events::span::SpanId;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_execute::materialize::materializer::HasMaterializer;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_execute::materialize::materializer::ReadLease;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::FinishedStarlarkEvaluation;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use buck2_util::time_span::TimeSpan;
use dice::CancellationContext;
use dice::DiceComputations;
use dice_futures::cancellation::CancellationObserver;
use dupe::Dupe;
use futures::FutureExt;
use smallvec::SmallVec;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::values::Heap;
use starlark::values::OwnedFrozenRef;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::AllocDict;
use starlark::values::dict::DictType;
use starlark::values::list::AllocList;
use starlark::values::tuple::AllocTuple;

use crate::attrs::DynamicAttrValue;
use crate::attrs::DynamicAttrValues;
use crate::bxl::eval_bxl_for_dynamic_output;
use crate::dynamic_actions_callable::FrozenStarlarkDynamicActionsCallable;
use crate::dynamic_actions_callable::P_ACTIONS;
use crate::params::FrozenDynamicLambdaParams;
use crate::resolved_dynamic_value::StarlarkResolvedDynamicValue;

/// The materialized inputs of a dynamic action, by artifact.
pub type ArtifactValues<'v> =
    ValueOf<'v, DictType<ValueTyped<'v, StarlarkArtifact>, ValueTyped<'v, StarlarkArtifactValue>>>;
/// The outputs of a dynamic action, declared on its own registry, by the artifact they stand for.
pub type Outputs<'v> = ValueOf<
    'v,
    DictType<ValueTyped<'v, StarlarkArtifact>, ValueTyped<'v, StarlarkDeclaredArtifact<'v>>>,
>;

pub enum DynamicLambdaArgs<'v> {
    OldPositional {
        ctx: Value<'v>,
        artifact_values: ArtifactValues<'v>,
        outputs: Outputs<'v>,
    },
    DynamicActionsNamed {
        actions: ValueTyped<'v, AnalysisActions<'v>>,
        attr_values: Box<[(String, Value<'v>)]>,
    },
    DynamicActionsBxlNamed {
        // cannot import BxlContext because it bxl is depends on this crate
        bxl_ctx: Value<'v>,
        attr_values: Box<[(String, Value<'v>)]>,
    },
}

pub fn invoke_dynamic_output_lambda<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    lambda: Value<'v>,
    args: DynamicLambdaArgs<'v>,
) -> buck2_error::Result<ProviderCollection<'v>> {
    let pos;
    let named;
    let (pos, named): (&[_], &[(_, _)]) = match &args {
        DynamicLambdaArgs::OldPositional {
            ctx,
            artifact_values,
            outputs,
        } => {
            pos = [*ctx, artifact_values.to_value(), outputs.to_value()];
            (&pos, &[])
        }
        DynamicLambdaArgs::DynamicActionsNamed {
            actions,
            attr_values,
        } => {
            named = iter::once((P_ACTIONS.name.as_str(), actions.to_value()))
                .chain(attr_values.iter().map(|(k, v)| (k.as_str(), *v)))
                .collect::<Vec<(&str, Value)>>();
            (&[], &named)
        }
        DynamicLambdaArgs::DynamicActionsBxlNamed {
            bxl_ctx,
            attr_values,
        } => {
            named = iter::once(("bxl_ctx", bxl_ctx.dupe()))
                .chain(attr_values.iter().map(|(k, v)| (k.as_str(), *v)))
                .collect::<Vec<(&str, Value)>>();
            (&[], &named)
        }
    };
    let return_value = eval.eval_function(lambda, pos, named)?;

    let provider_collection = match args {
        DynamicLambdaArgs::OldPositional { .. } => {
            if !return_value.is_none() {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "dynamic_output lambda must return `None`, got: `{0}`",
                    return_value.to_string_for_type_error()
                ));
            }
            ProviderCollection::try_from_value_dynamic_output(Value::new_empty_list())?
        }
        DynamicLambdaArgs::DynamicActionsNamed { .. } => {
            ProviderCollection::try_from_value_dynamic_output(return_value)?
        }
        DynamicLambdaArgs::DynamicActionsBxlNamed { .. } => {
            ProviderCollection::try_from_value_dynamic_output(return_value)?
        }
    };

    Ok(provider_collection)
}

fn execute_lambda_inner<'v>(
    env: &BuckStarlarkModule<'v>,
    eval_provider: StarlarkEvaluatorProvider,
    liveness: CancellationObserver,
    lambda: OwnedFrozenRef<'_, &'static FrozenDynamicLambdaParams<'static>>,
    self_key: &DynamicLambdaResultsKey,
    resolved_dynamic_values: BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
    ensured_artifacts: &BuckMutInsertionMap<&Artifact, &ArtifactValue>,
    input_artifacts_materialized: InputArtifactsMaterialized,
    digest_config: DigestConfig,
    artifact_fs: &ArtifactFs,
) -> buck2_error::Result<(FinishedStarlarkEvaluation, AnalysisRegistry<'v>)> {
    let print = EventDispatcherPrintHandler(get_dispatcher());
    eval_provider.with_evaluator(env, liveness.into(), |eval, _| {
        let heap = env.heap();
        eval.set_print_handler(&print);
        eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);
        let dynamic_lambda_ctx_data = dynamic_lambda_ctx_data(
            lambda,
            self_key.dupe(),
            input_artifacts_materialized,
            ensured_artifacts,
            &resolved_dynamic_values,
            artifact_fs,
            digest_config,
            env,
        )?;
        let ctx = AnalysisContext::prepare(
            heap,
            dynamic_lambda_ctx_data.lambda.attributes(),
            self_key.owner().configured_label(),
            dynamic_lambda_ctx_data.lambda.plugins(),
            dynamic_lambda_ctx_data.registry,
            dynamic_lambda_ctx_data.digest_config,
        );

        let args = match (
            &dynamic_lambda_ctx_data.lambda.attr_values,
            &dynamic_lambda_ctx_data.spec,
        ) {
            (
                None,
                DynamicLambdaCtxDataSpec::Old {
                    outputs,
                    artifact_values,
                },
            ) => DynamicLambdaArgs::OldPositional {
                ctx: ctx.to_value(),
                artifact_values: *artifact_values,
                outputs: *outputs,
            },
            (Some(_arg), DynamicLambdaCtxDataSpec::New { attr_values }) => {
                DynamicLambdaArgs::DynamicActionsNamed {
                    // TODO(nga): no need to create `ctx`
                    //   because we only need `actions` here.
                    actions: ctx.actions,
                    attr_values: attr_values.clone(),
                }
            }
            (None, DynamicLambdaCtxDataSpec::New { .. })
            | (Some(_), DynamicLambdaCtxDataSpec::Old { .. }) => {
                Err(internal_error!(
                    "Unexpected combination of attr_values and spec"
                ))?;
                unreachable!();
            }
        };

        let providers: ProviderCollection =
            invoke_dynamic_output_lambda(eval, dynamic_lambda_ctx_data.lambda.lambda(), args)?;
        let providers = eval.heap().alloc_typed(providers);

        ctx.assert_no_promises()?;

        let registry = ctx.take_state();

        registry
            .analysis_value_storage
            .set_result_value(providers)?;

        Ok(registry)
    })
}

async fn execute_lambda(
    lambda: OwnedFrozenRef<'_, &'static FrozenDynamicLambdaParams<'static>>,
    dice: &mut DiceComputations<'_>,
    self_key: DynamicLambdaResultsKey,
    resolved_dynamic_values: BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
    ensured_artifacts: &BuckMutInsertionMap<&Artifact, &ArtifactValue>,
    input_artifacts_materialized: InputArtifactsMaterialized,
    digest_config: DigestConfig,
    liveness: CancellationObserver,
) -> (
    TimeSpan,
    SmallVec<[SpanId; 1]>,
    buck2_error::Result<RecordedAnalysisValues>,
) {
    if let BaseDeferredKey::BxlLabel(key) = self_key.owner().dupe() {
        let res = eval_bxl_for_dynamic_output(
            &key,
            self_key,
            lambda,
            dice,
            input_artifacts_materialized,
            ensured_artifacts,
            resolved_dynamic_values,
            digest_config,
            liveness,
        )
        .await;
        (TimeSpan::empty_now(), SmallVec::new(), res)
    } else {
        let r = async {
            let artifact_fs = dice.get_artifact_fs().await?;
            let eval_kind = StarlarkEvalKind::DynamicOutput(Arc::new(self_key.dupe()));
            let eval_provider = StarlarkEvaluatorProvider::new(dice, eval_kind).await?;
            buck2_error::Ok((artifact_fs, eval_provider))
        }
        .await;
        let (artifact_fs, eval_provider) = match r {
            Ok(x) => x,
            Err(e) => {
                return (TimeSpan::empty_now(), SmallVec::new(), Err(e));
            }
        };

        let proto_rule = "dynamic_lambda".to_owned();

        let start_event = buck2_data::AnalysisStart {
            target: Some(buck2_data::analysis_start::Target::DynamicLambda(
                self_key.owner().to_proto().into(),
            )),
            rule: proto_rule.clone(),
        };

        let now = TimeSpan::start_now();

        let (res, spans) = record_root_spans(|| {
            span(start_event, || {
                let mut declared_actions = None;
                let mut declared_artifacts = None;

                let output: buck2_error::Result<_> = BuckStarlarkModule::with_profiling(|env| {
                    let (finished_evaluation, analysis_registry) = execute_lambda_inner(
                        &env,
                        eval_provider,
                        liveness,
                        lambda,
                        &self_key,
                        resolved_dynamic_values,
                        ensured_artifacts,
                        input_artifacts_materialized,
                        digest_config,
                        artifact_fs,
                    )?;

                    declared_actions = Some(analysis_registry.num_declared_actions());
                    declared_artifacts = Some(analysis_registry.num_declared_artifacts());
                    let registry_finalizer = analysis_registry.finalize(&env)?;
                    let (token, frozen_env, _) = finished_evaluation.freeze_and_finish(env)?;
                    let finalized = registry_finalizer(&frozen_env)?;
                    Ok((token, finalized))
                });

                let error = output.as_ref().err().map(|e| format!("{e:#}"));

                (
                    output,
                    buck2_data::AnalysisEnd {
                        target: Some(buck2_data::analysis_end::Target::DynamicLambda(
                            self_key.owner().to_proto().into(),
                        )),
                        rule: proto_rule,
                        profile: None,
                        declared_actions,
                        declared_artifacts,
                        error,
                    },
                )
            })
        });

        (now.end_now(), spans, res)
    }
}

pub async fn prepare_and_execute_lambda(
    ctx: &mut DiceComputations<'_>,
    cancellation: &CancellationContext,
    lambda: OwnedFrozenRef<'_, &'static FrozenDynamicLambdaParams<'static>>,
    self_holder_key: DynamicLambdaResultsKey,
) -> buck2_error::Result<RecordedAnalysisValues> {
    let mut waiting_data = WaitingData::new();
    // This is a bit suboptimal: we wait for all artifacts to be ready in order to
    // materialize any of them. However that is how we execute *all* local actions so in
    // the grand scheme of things that's probably not a huge deal.
    let all_artifact_group_values =
        ensure_artifacts_built(&lambda.value().static_fields.artifact_values, ctx).await?;
    let ensured_artifacts: BuckMutInsertionMap<_, _> = all_artifact_group_values
        .iter()
        .flat_map(|x| x.iter())
        .map(|(a, v)| (a, v))
        .collect();
    // Note: This may be an overapproximation because some of the deps could be overlapping, but
    // really these things shouldn't have deps anyway.
    let dynamic_inputs_bytes = ensured_artifacts.values().map(|v| v.size()).sum::<u64>();

    span_async_simple(
        buck2_data::DynamicLambdaStart {
            owner: Some(self_holder_key.owner().to_proto().into()),
            dynamic_inputs_bytes,
        },
        async move {
            waiting_data.start_waiting_category_now(WaitingCategory::MaterializingInputs);
            let ((input_artifacts_materialized, inputs_lease), resolved_dynamic_values) =
                span_async_simple(
                    buck2_data::DeferredPreparationStageStart {
                        stage: Some(buck2_data::MaterializedArtifacts {}.into()),
                    },
                    ctx.try_compute2(
                        async |ctx| materialize_inputs(&ensured_artifacts, ctx).await,
                        async |ctx| {
                            resolve_dynamic_values(
                                &lambda.value().static_fields.dynamic_values,
                                ctx,
                            )
                            .await
                        },
                    ),
                    buck2_data::DeferredPreparationStageEnd {},
                )
                .await?;
            // The lambda reads the materialized inputs while it runs.
            let _inputs_lease = inputs_lease;
            waiting_data.start_waiting_category_now(WaitingCategory::Unknown);
            let (time_span, spans, res) = cancellation
                .with_structured_cancellation(|observer| {
                    execute_lambda(
                        lambda,
                        ctx,
                        self_holder_key,
                        resolved_dynamic_values,
                        &ensured_artifacts,
                        input_artifacts_materialized,
                        ctx.global_data().get_digest_config(),
                        observer,
                    )
                    .boxed()
                })
                .await;
            ctx.store_evaluation_data(AnalysisKeyActivationData {
                time_span,
                spans,
                waiting_data,
                analysis_with_extra_data: AnalysisWithExtraData {
                    target_rule_type_name: None,
                },
                anon_target_split: None,
            })?;
            res
        },
        buck2_data::DynamicLambdaEnd {},
    )
    .await
}

async fn ensure_artifacts_built(
    materialized_artifacts: &[Artifact],
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<Vec<ArtifactGroupValues>> {
    if materialized_artifacts.is_empty() {
        return Ok(Vec::new());
    }

    ctx.try_compute_join(materialized_artifacts, async |ctx, artifact| {
        ctx.ensure_artifact_group(&ArtifactGroup::Artifact(artifact.dupe()))
            .await
    })
    .await
}

/// Marker to indicate artifacts we pass to dynamic actions are materialized.
/// To not forget to materialize after refactoring.
#[derive(Copy, Clone, Dupe)]
pub struct InputArtifactsMaterialized(());

async fn materialize_inputs(
    ensured_artifacts: &BuckMutInsertionMap<&Artifact, &ArtifactValue>,
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<(InputArtifactsMaterialized, ReadLease)> {
    if ensured_artifacts.is_empty() {
        return Ok((InputArtifactsMaterialized(()), ReadLease::noop()));
    }

    let artifact_fs = ctx.get_artifact_fs().await?;

    let mut artifacts = Vec::with_capacity(ensured_artifacts.len());

    for (artifact, artifact_value) in ensured_artifacts {
        let path = artifact.resolve_path(
            artifact_fs,
            if artifact.path_resolution_requires_artifact_value() {
                Some(artifact_value.content_based_path_hash())
            } else {
                None
            }
            .as_ref(),
        )?;
        artifacts.push((path, (*artifact_value).dupe()));
    }

    let re_use_case = invocation_re_use_case(ctx);
    let response = ctx
        .per_transaction_data()
        .get_materializer()
        .materialize(MaterializeRequest {
            artifacts,
            outputs: Vec::new(),
            purpose: MaterializationPurpose::IntermediateOnly,
            re_use_case,
        })
        .await?;
    for result in response.results {
        result?;
    }

    Ok((InputArtifactsMaterialized(()), response.lease))
}

async fn resolve_dynamic_values(
    dynamic_values: &[DynamicValue],
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<BuckMutMap<DynamicValue, FrozenProviderCollectionValue>> {
    if dynamic_values.is_empty() {
        return Ok(BuckMutMap::default());
    }

    let providers = ctx
        .try_compute_join(dynamic_values, async |ctx, dynamic_value| {
            let result = resolve_dynamic_value(dynamic_value, ctx).await?;
            buck2_error::Ok((dynamic_value.dupe(), result))
        })
        .await?;

    Ok(BuckMutMap::from_iter(providers))
}

/// Evaluates the dynamic lambda that produces `dynamic_value`, including building its input
/// artifacts and resolving the dynamic values it depends on, and returns its providers.
pub async fn resolve_dynamic_value(
    dynamic_value: &DynamicValue,
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<FrozenProviderCollectionValue> {
    Ok(
        dynamic_lambda_result(ctx, &dynamic_value.dynamic_lambda_results_key)
            .await?
            .analysis_values
            .provider_collection()?
            .to_owned(),
    )
}

pub enum DynamicLambdaCtxDataSpec<'v> {
    Old {
        outputs: Outputs<'v>,
        artifact_values: ArtifactValues<'v>,
    },
    New {
        attr_values: Box<[(String, Value<'v>)]>,
    },
}

/// Data used to construct an `AnalysisContext` or `BxlContext` for the dynamic lambda.
pub struct DynamicLambdaCtxData<'v> {
    pub lambda: &'v FrozenDynamicLambdaParams<'v>,
    pub spec: DynamicLambdaCtxDataSpec<'v>,
    pub digest_config: DigestConfig,
    pub registry: AnalysisRegistry<'v>,
}

/// Prepare dict of artifact values for dynamic actions.
fn artifact_values<'v>(
    ensured_artifacts: &BuckMutInsertionMap<&Artifact, &ArtifactValue>,
    _: InputArtifactsMaterialized,
    artifact_fs: &ArtifactFs,
    heap: Heap<'v>,
) -> buck2_error::Result<ArtifactValues<'v>> {
    let mut artifact_values_dict = Vec::with_capacity(ensured_artifacts.len());
    for (artifact, artifact_value) in ensured_artifacts.iter() {
        let artifact = *artifact;
        let k = StarlarkArtifact::new(artifact.dupe());
        let path = artifact.get_path().resolve(
            artifact_fs,
            if artifact.path_resolution_requires_artifact_value() {
                Some(artifact_value.content_based_path_hash())
            } else {
                None
            }
            .as_ref(),
        )?;
        // `InputArtifactsMaterialized` marker indicates that the artifact is materialized.
        let v =
            StarlarkArtifactValue::new(artifact.dupe(), path.to_owned(), artifact_fs.fs().dupe());
        artifact_values_dict.push((k, v));
    }
    Ok(ValueOf::alloc_dict(
        heap,
        artifact_values_dict.into_iter().map(|(k, v)| {
            (
                ValueOf::from(heap.alloc_typed(k)),
                ValueOf::from(heap.alloc_typed(v)),
            )
        }),
    ))
}

/// Prepare dict of output artifacts for dynamic actions.
fn outputs<'v>(
    outputs: &[ValueTyped<'v, FrozenStarlarkOutputArtifact<'v>>],
    registry: &mut AnalysisRegistry<'v>,
    heap: Heap<'v>,
) -> buck2_error::Result<Outputs<'v>> {
    let mut outputs_dict = Vec::with_capacity(outputs.len());
    for x in outputs {
        let declared = registry.declare_dynamic_output(x.as_ref().as_build_artifact(), heap)?;
        let v = StarlarkDeclaredArtifact::new(None, declared, AssociatedArtifacts::new());
        outputs_dict.push((
            ValueOf::from(x.as_ref().inner()),
            ValueOf::from(heap.alloc_typed(v)),
        ));
    }

    Ok(ValueOf::alloc_dict(heap, outputs_dict))
}

fn new_attr_value<'v>(
    value: &DynamicAttrValue<'v>,
    _input_artifacts_materialized: InputArtifactsMaterialized,
    ensured_artifacts: &BuckMutInsertionMap<&Artifact, &ArtifactValue>,
    artifact_fs: &ArtifactFs,
    registry: &mut AnalysisRegistry<'v>,
    resolved_dynamic_values: &BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
    env: &Module<'v>,
) -> buck2_error::Result<Value<'v>> {
    match value {
        DynamicAttrValue::Output(artifact) => {
            let artifact = match artifact.unpack() {
                StarlarkOutputArtifactView::Frozen(artifact) => {
                    artifact.as_ref().as_build_artifact()
                }
                StarlarkOutputArtifactView::Unfrozen(_) => {
                    return Err(internal_error!(
                        "Dynamic action attributes are frozen before the action runs"
                    ));
                }
            };
            let declared = registry.declare_dynamic_output(artifact, env.heap())?;
            let artifact = env.heap().alloc_typed(StarlarkDeclaredArtifact::new(
                None,
                declared,
                AssociatedArtifacts::new(),
            ));
            Ok(env.heap().alloc(StarlarkOutputArtifact::new(artifact)))
        }
        DynamicAttrValue::ArtifactValue(artifact) => {
            let path = artifact.get_path().resolve(
                artifact_fs,
                if artifact.path_resolution_requires_artifact_value() {
                    Some(
                        ensured_artifacts
                            .get(artifact)
                            .ok_or_else(|| {
                                internal_error!("Dynamic action missing input artifact value!")
                            })?
                            .content_based_path_hash(),
                    )
                } else {
                    None
                }
                .as_ref(),
            )?;
            // `InputArtifactsMaterialized` marker indicates that the artifact is materialized.
            Ok(env.heap().alloc(StarlarkArtifactValue::new(
                artifact.dupe(),
                path.to_owned(),
                artifact_fs.fs().dupe(),
            )))
        }
        DynamicAttrValue::DynamicValue(v) => {
            let v = resolved_dynamic_values
                .get(v)
                .internal_error("Missing resolved dynamic value")?;
            Ok(env.heap().alloc(StarlarkResolvedDynamicValue {
                value: v.add_heap_ref(env.heap()),
            }))
        }
        DynamicAttrValue::Value(v) => Ok(*v),
        DynamicAttrValue::List(xs) => {
            let xs = xs
                .iter()
                .map(|x| {
                    new_attr_value(
                        x,
                        _input_artifacts_materialized,
                        ensured_artifacts,
                        artifact_fs,
                        registry,
                        resolved_dynamic_values,
                        env,
                    )
                })
                .collect::<buck2_error::Result<Vec<_>>>()?;
            Ok(env.heap().alloc(AllocList(xs)))
        }
        DynamicAttrValue::Dict(xs) => {
            let mut r = BuckMutInsertionMap::with_capacity(xs.len());
            for (k, v) in xs.iter() {
                let prev = r.insert_hashed(
                    k.get_hashed()?,
                    new_attr_value(
                        v,
                        _input_artifacts_materialized,
                        ensured_artifacts,
                        artifact_fs,
                        registry,
                        resolved_dynamic_values,
                        env,
                    )?,
                );
                if prev.is_some() {
                    return Err(buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Duplicate key in dict"
                    ));
                }
            }
            Ok(env.heap().alloc(AllocDict(r)))
        }
        DynamicAttrValue::Tuple(xs) => {
            let xs = xs
                .iter()
                .map(|x| {
                    new_attr_value(
                        x,
                        _input_artifacts_materialized,
                        ensured_artifacts,
                        artifact_fs,
                        registry,
                        resolved_dynamic_values,
                        env,
                    )
                })
                .collect::<buck2_error::Result<Vec<_>>>()?;
            Ok(env.heap().alloc(AllocTuple(xs)))
        }
        DynamicAttrValue::Option(option) => match option {
            Some(v) => new_attr_value(
                v,
                _input_artifacts_materialized,
                ensured_artifacts,
                artifact_fs,
                registry,
                resolved_dynamic_values,
                env,
            ),
            None => Ok(Value::new_none()),
        },
    }
}

fn new_attr_values<'v>(
    values: &DynamicAttrValues<'v>,
    callable: &FrozenStarlarkDynamicActionsCallable<'_>,
    input_artifacts_materialized: InputArtifactsMaterialized,
    ensured_artifacts: &BuckMutInsertionMap<&Artifact, &ArtifactValue>,
    artifact_fs: &ArtifactFs,
    registry: &mut AnalysisRegistry<'v>,
    resolved_dynamic_values: &BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
    env: &Module<'v>,
) -> buck2_error::Result<Box<[(String, Value<'v>)]>> {
    if values.values.len() != callable.attrs.len() {
        return Err(internal_error!("Parameter count mismatch"));
    }
    callable
        .attrs
        .keys()
        .zip(values.values.iter())
        .map(|(name, value)| {
            Ok((
                name.clone(),
                new_attr_value(
                    value,
                    input_artifacts_materialized,
                    ensured_artifacts,
                    artifact_fs,
                    registry,
                    resolved_dynamic_values,
                    env,
                )?,
            ))
        })
        .collect()
}

/// Sets up the data needed to create the dynamic lambda ctx and evaluate the lambda.
pub fn dynamic_lambda_ctx_data<'v>(
    dynamic_lambda: OwnedFrozenRef<'_, &'static FrozenDynamicLambdaParams<'static>>,
    self_key: DynamicLambdaResultsKey,
    input_artifacts_materialized: InputArtifactsMaterialized,
    ensured_artifacts: &BuckMutInsertionMap<&Artifact, &ArtifactValue>,
    resolved_dynamic_values: &BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
    artifact_fs: &ArtifactFs,
    digest_config: DigestConfig,
    env: &Module<'v>,
) -> buck2_error::Result<DynamicLambdaCtxData<'v>> {
    let self_key = Arc::new(self_key);

    let dynamic_lambda = dynamic_lambda.add_to_heap(env.heap());

    let mut registry = AnalysisRegistry::new_from_owner_and_deferred(
        dynamic_lambda.static_fields.execution_platform.dupe(),
        DeferredHolderKey::DynamicLambda(self_key),
    )?;

    let spec = match &dynamic_lambda.attr_values {
        None => {
            let artifact_values = artifact_values(
                ensured_artifacts,
                input_artifacts_materialized,
                artifact_fs,
                env.heap(),
            )?;
            let outputs = outputs(&dynamic_lambda.outputs, &mut registry, env.heap())?;
            if !dynamic_lambda.static_fields.dynamic_values.is_empty() {
                return Err(internal_error!(
                    "Non-empty `dynamic_value` for `dynamic_output`"
                ));
            }
            DynamicLambdaCtxDataSpec::Old {
                outputs,
                artifact_values,
            }
        }
        Some((attr_values, callable)) => DynamicLambdaCtxDataSpec::New {
            attr_values: new_attr_values(
                attr_values,
                callable.as_ref(),
                input_artifacts_materialized,
                ensured_artifacts,
                artifact_fs,
                &mut registry,
                resolved_dynamic_values,
                env,
            )?,
        },
    };

    Ok(DynamicLambdaCtxData {
        lambda: dynamic_lambda,
        spec,
        digest_config,
        registry,
    })
}
