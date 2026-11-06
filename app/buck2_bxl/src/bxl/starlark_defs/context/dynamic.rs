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
use std::sync::LazyLock;

use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::analysis::registry::RecordedAnalysisValues;
use buck2_build_api::dynamic_value::DynamicValue;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_common::events::HasEvents;
use buck2_common::scope::scope_and_collect_with_dice;
use buck2_core::deferred::base_deferred_key::BaseDeferredKeyBxl;
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_dynamic_actions::attrs_starlark::StarlarkDynamicAttrType;
use buck2_dynamic_actions::bxl::EVAL_BXL_FOR_DYNAMIC_OUTPUT;
use buck2_dynamic_actions::deferred::DynamicLambdaArgs;
use buck2_dynamic_actions::deferred::DynamicLambdaCtxDataSpec;
use buck2_dynamic_actions::deferred::InputArtifactsMaterialized;
use buck2_dynamic_actions::deferred::dynamic_lambda_ctx_data;
use buck2_dynamic_actions::deferred::invoke_dynamic_output_lambda;
use buck2_dynamic_actions::dynamic_actions_callable::DynamicActionsCallable;
use buck2_dynamic_actions::dynamic_actions_callable::DynamicActionsCallbackParam;
use buck2_dynamic_actions::dynamic_actions_callable::DynamicActionsCallbackParamSpec;
use buck2_dynamic_actions::dynamic_actions_callable::DynamicActionsCallbackReturnType;
use buck2_dynamic_actions::new_dynamic_actions_callable;
use buck2_dynamic_actions::params::FrozenDynamicLambdaParams;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use dice::DiceComputations;
use dice_futures::cancellation::CancellationObserver;
use dupe::Dupe;
use itertools::Itertools;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::OwnedFrozenRef;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::type_repr::StarlarkTypeRepr;
use starlark::values::typing::StarlarkCallableChecked;

use crate::bxl::eval::LIMITED_EXECUTOR;
use crate::bxl::key::BxlDynamicKey;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::BxlContextCoreData;
use crate::bxl::starlark_defs::context::DynamicBxlContextData;
use crate::bxl::starlark_defs::context::starlark_async::BxlDiceComputations;
use crate::bxl::starlark_defs::eval_extra::BxlEvalExtra;

pub(crate) async fn eval_bxl_for_dynamic_output<'v>(
    base_deferred_key: &'v BaseDeferredKeyBxl,
    self_key: DynamicLambdaResultsKey,
    dynamic_lambda: OwnedFrozenRef<'v, &'static FrozenDynamicLambdaParams<'static>>,
    dice_ctx: &'v mut DiceComputations<'_>,
    input_artifacts_materialized: InputArtifactsMaterialized,
    ensured_artifacts: &'v BuckMutInsertionMap<&'v Artifact, &'v ArtifactValue>,
    resolved_dynamic_values: BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
    _digest_config: DigestConfig,
    liveness: CancellationObserver,
) -> buck2_error::Result<RecordedAnalysisValues> {
    // TODO(wendyy) emit telemetry, support profiler
    let dynamic_key =
        BxlDynamicKey::from_base_deferred_key_dyn_impl_err(base_deferred_key.clone())?;
    let key = dynamic_key.key();
    // FIXME(JakobDegen): Audit that this string is user-friendly.
    // Currently uses BxlKey's Display, which formats as "{bxl_path}:{name}"
    // (e.g. "cell//path/to/file.bxl:function_name").
    let eval_kind = StarlarkEvalKind::BxlDynamic(key.to_string().into());
    let dynamic_data = DynamicBxlContextData {
        exec_deps: dynamic_key
            .0
            .execution_resolution
            .exec_deps_configured
            .clone(),
        toolchains: dynamic_key
            .0
            .execution_resolution
            .toolchain_deps_configured
            .clone(),
    };
    // TODO(cjhopman): Why does this get the digest_config from dice???
    let digest_config = dice_ctx.global_data().get_digest_config();
    let dispatcher = dice_ctx.per_transaction_data().get_dispatcher().dupe();
    let artifact_fs = dice_ctx.get_artifact_fs().await?;
    let eval_ctx = BxlDynamicOutputEvaluator {
        data: BxlContextCoreData::new(key, dice_ctx).await?,
        self_key,
        liveness,
        dynamic_lambda,
        dynamic_data,
        digest_config,
        input_artifacts_materialized,
        ensured_artifacts,
        resolved_dynamic_values,
        artifact_fs: artifact_fs.dupe(),

        print: EventDispatcherPrintHandler(dispatcher.dupe()),
    };

    let limited_executor = LIMITED_EXECUTOR.clone();

    // Note: because we use `block_in_place`, that will prevent the inner future from being polled
    // and yielded. So, for cancellation observers to work properly within the dice cancellable
    // future context, we need the future that it's attached to the cancellation context can
    // yield and be polled. To ensure that, we have to spawn the future that then enters block_in_place
    let (_, futs) = unsafe {
        // SAFETY: as long as we don't `forget` the return object from `scope_and_collect`, it is safe

        // Additional cancellation notes:
        // the `scope_and_collect` will block on drop, but it will move the blocking to a tokio
        // blocking thread, freeing up the main worker threads. Additionally, the `spawn_cancellable`
        // on the scope will be dropped at the earliest await point. If we are within the blocking
        // section of bxl, the cancellation observer will be notified and cause the blocking calls
        // to terminate.
        scope_and_collect_with_dice(dice_ctx, |dice_ctx, s| {
            s.spawn_cancellable(
                limited_executor.execute(async move {
                    let eval_provider = StarlarkEvaluatorProvider::new(dice_ctx, eval_kind).await?;
                    tokio::task::block_in_place(|| eval_ctx.do_eval(eval_provider, dice_ctx))
                }),
                || Err(buck2_error!(buck2_error::ErrorTag::Tier0, "cancelled")),
            )
        })
    }
    .await;

    match futs.into_iter().exactly_one() {
        Ok(res) => Ok(res??),
        Err(_) => panic!("only spawned one task"),
    }
}

struct BxlDynamicOutputEvaluator<'f> {
    data: BxlContextCoreData,
    self_key: DynamicLambdaResultsKey,
    liveness: CancellationObserver,
    dynamic_lambda: OwnedFrozenRef<'f, &'static FrozenDynamicLambdaParams<'static>>,
    dynamic_data: DynamicBxlContextData,
    digest_config: DigestConfig,
    input_artifacts_materialized: InputArtifactsMaterialized,
    ensured_artifacts: &'f BuckMutInsertionMap<&'f Artifact, &'f ArtifactValue>,
    resolved_dynamic_values: BuckMutMap<DynamicValue, FrozenProviderCollectionValue>,
    artifact_fs: ArtifactFs,
    print: EventDispatcherPrintHandler,
}

impl BxlDynamicOutputEvaluator<'_> {
    fn do_eval(
        self,
        provider: StarlarkEvaluatorProvider,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<RecordedAnalysisValues> {
        BuckStarlarkModule::with_profiling(|env| {
            let bxl_dice = BxlDiceComputations::new(dice, self.liveness.dupe());

            let (finished_eval, analysis_registry) = {
                let data = Arc::new(self.data);
                let mut extra = BxlEvalExtra::new_dynamic(bxl_dice, data.dupe());
                provider.with_evaluator(&env, self.liveness.into(), |eval, _| {
                    eval.set_print_handler(&self.print);
                    eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);
                    eval.extra_mut = Some(&mut extra);

                    let dynamic_lambda_ctx_data = dynamic_lambda_ctx_data(
                        self.dynamic_lambda,
                        self.self_key.dupe(),
                        self.input_artifacts_materialized,
                        self.ensured_artifacts,
                        &self.resolved_dynamic_values,
                        &self.artifact_fs,
                        self.digest_config,
                        &env,
                    )?;

                    let bxl_dynamic_ctx = BxlContext::new_dynamic(
                        env.heap(),
                        data,
                        self.digest_config,
                        dynamic_lambda_ctx_data.registry,
                        self.dynamic_data,
                    )?;

                    let ctx = ValueTyped::<BxlContext>::new_err(env.heap().alloc(bxl_dynamic_ctx))?;

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
                            DynamicLambdaArgs::DynamicActionsBxlNamed {
                                bxl_ctx: ctx.to_value(),
                                attr_values: attr_values.clone(),
                            }
                        }
                        (None, DynamicLambdaCtxDataSpec::New { .. })
                        | (Some(_), DynamicLambdaCtxDataSpec::Old { .. }) => {
                            return Err(internal_error!("Inconsistent"));
                        }
                    };

                    invoke_dynamic_output_lambda(
                        eval,
                        dynamic_lambda_ctx_data.lambda.lambda(),
                        args,
                    )?;

                    ctx.take_state_dynamic()
                })?
            };

            let finalizer = analysis_registry.finalize(&env)?;

            let (token, frozen_env, _) = finished_eval.freeze_and_finish(env)?;
            let recorded_values = finalizer(&frozen_env)?;

            Ok((token, recorded_values))
        })
    }
}

pub(crate) fn init_eval_bxl_for_dynamic_output() {
    EVAL_BXL_FOR_DYNAMIC_OUTPUT.init(
        |base_deferred_key,
         self_key,
         dynamic_lambda,
         dice_ctx,
         input_artifacts_materialized,
         ensured_artifacts,
         resolved_dynamic_values,
         digest_config,
         liveness| {
            Box::pin(eval_bxl_for_dynamic_output(
                base_deferred_key,
                self_key,
                dynamic_lambda,
                dice_ctx,
                input_artifacts_materialized,
                ensured_artifacts,
                resolved_dynamic_values,
                digest_config,
                liveness,
            ))
        },
    );
}

pagable::static_str!(P_BXLCTX_NAME = "bxl_ctx");

static P_BXLCTX: DynamicActionsCallbackParam = DynamicActionsCallbackParam {
    name: P_BXLCTX_NAME,
    ty: LazyLock::new(BxlContext::starlark_type_repr),
};

#[starlark_module]
pub(crate) fn register_dynamic_actions(globals: &mut GlobalsBuilder) {
    /// Create new bxl dynamic action callable. Returned object will be callable,
    /// and the result of calling it can be passed to `ctx.actions.dynamic_output_new`.
    ///
    /// Be aware that the context argument of the called impl function differs between
    /// [`dynamic_actions`](../build/#dynamic_actions) where it is [`actions: AnalysisActions`](../build/AnalysisActions)
    /// and [`bxl.dynamic_actions`](#dynamic_actions)
    /// where it is [`bxl_ctx: bxl.Context`](../bxl/Context).
    fn dynamic_actions<'v>(
        #[starlark(require = named)] r#impl: StarlarkCallableChecked<
            'v,
            DynamicActionsCallbackParamSpec,
            DynamicActionsCallbackReturnType,
        >,
        #[starlark(require = named)] attrs: ValueOf<
            'v,
            DictType<String, &'v StarlarkDynamicAttrType<'v>>,
        >,
    ) -> starlark::Result<DynamicActionsCallable<'v>> {
        Ok(new_dynamic_actions_callable(r#impl, attrs, &P_BXLCTX)?)
    }
}
