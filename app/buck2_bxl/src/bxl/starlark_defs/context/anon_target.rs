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

use buck2_analysis::analysis::calculation::get_loaded_module;
use buck2_analysis::analysis::env::get_rule_callable;
use buck2_analysis::analysis::env::get_rule_impl;
use buck2_analysis::analysis::env::promise_artifact_mappings;
use buck2_analysis::analysis::env::transitive_validations;
use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::analysis::anon_promises_dyn::RunAnonPromisesAccessor;
use buck2_build_api::analysis::registry::AnalysisRegistry;
use buck2_build_api::anon_target::AnonTargetDependentAnalysisResults;
use buck2_build_api::anon_target::AnonTargetDyn;
use buck2_build_api::bxl::anon_target::EVAL_BXL_FOR_ANON_TARGET;
use buck2_build_api::bxl::types::BxlFunctionLabel;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_build_api::interpreter::rule_defs::provider::ty::abstract_provider::AbstractProvider;
use buck2_common::events::HasEvents;
use buck2_common::scope::scope_and_collect_with_dice;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_hash::BuckFixedInsertionMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use buck2_interpreter_for_build::attrs::StarlarkAttribute;
use buck2_interpreter_for_build::rule::StarlarkRuleCallable;
use buck2_interpreter_for_build::rule::frozen_rule_attribute_spec;
use buck2_node::attrs::spec::AttributeSpec;
use buck2_node::bzl_or_bxl_path::BzlOrBxlPath;
use buck2_node::rule_type::RuleType;
use dice::DiceComputations;
use dice_futures::cancellation::CancellationObserver;
use dupe::Dupe;
use itertools::Itertools;
use starlark::environment::FrozenModule;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::typing::ParamIsRequired;
use starlark::typing::ParamSpec;
use starlark::util::ArcStr;
use starlark::values::StringValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::list::ListType;
use starlark::values::structs::StructRef;
use starlark::values::type_repr::StarlarkTypeRepr;
use starlark::values::typing::StarlarkCallable;
use starlark::values::typing::StarlarkCallableChecked;
use starlark::values::typing::StarlarkCallableParamSpec;

use crate::bxl::eval::LIMITED_EXECUTOR;
use crate::bxl::key::BxlKey;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::BxlContextCoreData;
use crate::bxl::starlark_defs::context::BxlDiceComputations;
use crate::bxl::starlark_defs::eval_extra::BxlEvalExtra;

struct BxlAnonCallbackParamSpec;

pagable::static_str!(BXL_ANON_BXL_CTX = "bxl_ctx");
pagable::static_str!(BXL_ANON_ATTRS = "attrs");

impl StarlarkCallableParamSpec for BxlAnonCallbackParamSpec {
    fn params() -> ParamSpec {
        ParamSpec::new_parts(
            [],
            [],
            None,
            [
                (
                    ArcStr::new_static(BXL_ANON_BXL_CTX),
                    ParamIsRequired::Yes,
                    BxlContext::starlark_type_repr(),
                ),
                (
                    ArcStr::new_static(BXL_ANON_ATTRS),
                    ParamIsRequired::Yes,
                    StructRef::starlark_type_repr(),
                ),
            ],
            None,
        )
        .unwrap()
    }
}

#[starlark_module]
pub(crate) fn register_anon_rule(globals: &mut GlobalsBuilder) {
    /// Create a new anonymous rule.
    fn anon_rule<'v>(
        #[starlark(require = named)] r#impl: StarlarkCallableChecked<
            'v,
            BxlAnonCallbackParamSpec,
            ListType<AbstractProvider>,
        >,
        #[starlark(require = named)] attrs: ValueOf<'v, DictType<&'v str, &'v StarlarkAttribute>>,
        #[starlark(require = named, default = "")] doc: &str,
        #[starlark(require = named, default = ValueOf::empty_dict())]
        artifact_promise_mappings: ValueOf<
            'v,
            DictType<
                StringValue<'v>,
                StarlarkCallable<'v, (Value<'static>,), ListType<Value<'static>>>,
            >,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkRuleCallable<'v>> {
        StarlarkRuleCallable::new_bxl_anon(
            StarlarkCallable::unchecked_new(r#impl.0),
            attrs,
            doc,
            artifact_promise_mappings,
            eval,
        )
        .map_err(Into::into)
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum BxlAnonTargetError {
    #[error("Anon BXL is not supported in bzl files")]
    UnsupportedInBzl,
}

/// `RuleSpec` equivalent for `bxl.anon_target`.
struct AnonImpl {
    module: FrozenModule,
    name: String,
}

impl AnonImpl {
    async fn new(
        dice: &mut DiceComputations<'_>,
        anon_target: Arc<dyn AnonTargetDyn>,
    ) -> buck2_error::Result<Self> {
        let rule_type = anon_target.rule_type();
        let module = get_loaded_module(dice, rule_type).await?;
        Ok(Self {
            module: module.env().dupe(),
            name: rule_type.name.clone(),
        })
    }

    fn invoke<'v>(
        &self,
        eval: &mut Evaluator<'v, '_, '_>,
        bxl_ctx: ValueTyped<'v, BxlContext<'v>>,
        attrs: ValueOf<'v, StructRef<'v>>,
    ) -> buck2_error::Result<Value<'v>> {
        let anon_impl = get_rule_impl(eval, &self.module, &self.name)?;
        eval.eval_function(anon_impl, &[bxl_ctx.to_value(), attrs.to_value()], &[])
            .map_err(|e| e.into())
    }

    fn promise_artifact_mappings<'v>(
        &self,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<BuckMutInsertionMap<String, Value<'v>>> {
        promise_artifact_mappings(eval, &self.module, &self.name)
    }

    fn attribute_spec<'v>(
        &self,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<&'v AttributeSpec> {
        frozen_rule_attribute_spec(get_rule_callable(eval, &self.module, &self.name)?)
    }
}

async fn eval_bxl_for_anon_target(
    dice: &mut DiceComputations<'_>,
    anon_target: Arc<dyn AnonTargetDyn>,
    global_cfg_options: GlobalCfgOptions,
    dependents_analyses: AnonTargetDependentAnalysisResults<'_>,
    execution_platform: ExecutionPlatformResolution,
    liveness: CancellationObserver,
) -> buck2_error::Result<AnalysisResult> {
    // Note: because we use `block_in_place`, that will prevent the inner future from being polled
    // and yielded. So, for cancellation observers to work properly within the dice cancellable
    // future context, we need the future that it's attached to the cancellation context can
    // yield and be polled. To ensure that, we have to spawn the future that then enters block_in_place

    let limited_executor = LIMITED_EXECUTOR.clone();

    let (_, futs) = unsafe {
        // SAFETY: as long as we don't `forget` the return object from `scope_and_collect`, it is safe

        // Additional cancellation notes:
        // the `scope_and_collect` will block on drop, but it will move the blocking to a tokio
        // blocking thread, freeing up the main worker threads. Additionally, the `spawn_cancellable`
        // on the scope will be dropped at the earliest await point. If we are within the blocking
        // section of bxl, the cancellation observer will be notified and cause the blocking calls
        // to terminate.
        scope_and_collect_with_dice(dice, |dice, s| {
            s.spawn_cancellable(
                limited_executor.execute(eval_bxl_for_anon_target_inner(
                    dice,
                    anon_target,
                    global_cfg_options,
                    dependents_analyses,
                    execution_platform,
                    liveness,
                )),
                || Err(buck2_error!(buck2_error::ErrorTag::Tier0, "cancelled")),
            )
        })
    }
    .await;
    match futs.into_iter().exactly_one() {
        Ok(res) => res?,
        Err(_) => panic!("only spawned one task"),
    }
}

async fn eval_bxl_for_anon_target_inner(
    dice: &mut DiceComputations<'_>,
    anon_target: Arc<dyn AnonTargetDyn>,
    global_cfg_options: GlobalCfgOptions,
    dependents_analyses: AnonTargetDependentAnalysisResults<'_>,
    execution_platform: ExecutionPlatformResolution,
    liveness: CancellationObserver,
) -> buck2_error::Result<AnalysisResult> {
    let rule_type = anon_target.rule_type();
    let bxl_spec = match &rule_type.path {
        BzlOrBxlPath::Bxl(bxl_file_path) => BxlFunctionLabel {
            bxl_path: bxl_file_path.clone(),
            name: rule_type.name.clone(),
        },
        BzlOrBxlPath::Bzl(_) => return Err(BxlAnonTargetError::UnsupportedInBzl.into()),
    };
    let bxl_key = BxlKey::new(
        bxl_spec,
        Arc::new(BuckFixedInsertionMap::new()),
        false,
        global_cfg_options,
    );
    let bxl_ctx_core_data = BxlContextCoreData::new(bxl_key.dupe(), dice).await?;

    let dispatcher = dice.per_transaction_data().get_dispatcher().dupe();
    let print = EventDispatcherPrintHandler(dispatcher);
    let digest_config = dice.global_data().get_digest_config();

    let validations_from_deps = dependents_analyses.validations();

    let anon_impl = AnonImpl::new(dice, anon_target.dupe()).await?;

    let eval_kind = anon_target.dupe().eval_kind();
    let provider = StarlarkEvaluatorProvider::new(dice, eval_kind).await?;

    BuckStarlarkModule::with_profiling(|env| {
        let bxl_dice = BxlDiceComputations::new(dice, liveness.dupe());
        let bxl_ctx_core_data = Arc::new(bxl_ctx_core_data);
        let mut extra = BxlEvalExtra::new_anon(bxl_dice, bxl_ctx_core_data.dupe());

        let mut reentrant_eval = provider.make_reentrant_evaluator(&env, liveness.into())?;
        let (bxl_ctx, list_res) = reentrant_eval.with_evaluator(|eval| {
            eval.set_print_handler(&print);
            eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);
            eval.extra_mut = Some(&mut extra);

            let analysis_registry = AnalysisRegistry::new_from_owner(
                anon_target.dupe().base_deferred_key(),
                execution_platform.clone(),
            )?;

            let attrs_spec = anon_impl.attribute_spec(eval)?;
            let attributes = anon_target.resolve_attrs(
                &env,
                attrs_spec,
                dependents_analyses,
                execution_platform.clone(),
            )?;

            let bxl_anon_ctx = BxlContext::new_anon(
                env.heap(),
                bxl_ctx_core_data,
                digest_config,
                analysis_registry,
                attributes,
            )?;
            let bxl_ctx = ValueTyped::<BxlContext>::new_err(env.heap().alloc(bxl_anon_ctx))?;

            let list_res = tokio::task::block_in_place(|| -> buck2_error::Result<Value<'_>> {
                anon_impl.invoke(eval, bxl_ctx, attributes)
            })?;

            Ok((bxl_ctx, list_res))
        })?;

        let action_factory = bxl_ctx.state;

        let requested_anon_targets = tokio::task::block_in_place(|| {
            reentrant_eval
                .with_evaluator(|eval| run_anon_target_promises(action_factory, &bxl_ctx, eval))
        })?;

        let res_typed = ProviderCollection::try_from_value(list_res)?;
        let res = env.heap().alloc(res_typed);

        let fulfilled_artifact_mappings = reentrant_eval.with_evaluator(|eval| {
            let promise_artifact_mappings = anon_impl.promise_artifact_mappings(eval)?;

            anon_target
                .dupe()
                .get_fulfilled_promise_artifacts(promise_artifact_mappings, res, eval)
        })?;

        let res = ValueTyped::new(res).internal_error("Just allocated the provider collection")?;

        let analysis_registry = bxl_ctx.take_state_anon()?;
        analysis_registry
            .analysis_value_storage
            .set_result_value(res)?;

        let finished_eval = reentrant_eval.finish_evaluation();
        std::mem::drop(extra);

        let num_declared_actions = analysis_registry.num_declared_actions();
        let num_declared_artifacts = analysis_registry.num_declared_artifacts();
        let registry_finalizer = analysis_registry.finalize(&env)?;
        let (token, frozen_env, _) = finished_eval.freeze_and_finish(env)?;
        let recorded_values = registry_finalizer(&frozen_env)?;

        let validations = transitive_validations(
            validations_from_deps,
            recorded_values.provider_collection()?,
        )?;

        Ok((
            token,
            AnalysisResult::new(
                recorded_values,
                None,
                fulfilled_artifact_mappings,
                num_declared_actions,
                num_declared_artifacts,
                validations,
                requested_anon_targets,
                RuleType::Starlark(anon_target.rule_type().dupe()),
            )?,
        ))
    })
}

struct BxlAnonPromisesAccessor<'me, 'v, 'a, 'e>(
    &'me mut Evaluator<'v, 'a, 'e>,
    &'me BxlContext<'v>,
);

impl<'me, 'v, 'a, 'e> RunAnonPromisesAccessor<'v, 'a, 'e>
    for BxlAnonPromisesAccessor<'me, 'v, 'a, 'e>
{
    fn with_evaluator(
        &mut self,
        closure: &mut dyn FnMut(&mut Evaluator<'v, 'a, 'e>) -> buck2_error::Result<()>,
    ) -> buck2_error::Result<()> {
        closure(self.0)
    }

    fn via_dice_impl<'s: 'b, 'b>(
        &'s mut self,
        f: Box<dyn for<'d> FnOnce(&'s mut DiceComputations<'d>) + 'b>,
    ) {
        self.1.via_dice(self.0, |dice| dice.with_inner_less_safe(f))
    }
}

/// Returns the anon target keys requested by the resolved promises.
pub(crate) fn run_anon_target_promises<'v, 'a, 'e>(
    actions: ValueTyped<'v, AnalysisActions<'v>>,
    ctx: &BxlContext<'v>,
    eval: &mut Evaluator<'v, 'a, 'e>,
) -> buck2_error::Result<Vec<BaseDeferredKey>> {
    let mut accessor = BxlAnonPromisesAccessor(eval, ctx);
    // TODO(cjhopman): The approach here is pretty against the general model that we want. Ideally
    // we'd like to split this into two steps:
    //  1. Get values needed for running promises from dice
    //  2. Run promise mappings here
    //
    // But the weirdness of the promise mappings means we can't really do that.
    //
    // Doing this and using `with_inner_less_safe` above is a practical workaround, but it comes
    // with disadvantages, basically that it does not respect the "safety" of the standard
    // `via_dice` thing. Concretely, that means it doesn't respect cancellations and fails to report
    // a proper span for dice access.
    tokio::runtime::Handle::current()
        .block_on(actions.run_promises(&mut accessor))
        .map(|run| run.requested_anon_targets)
}

pub(crate) fn init_eval_bxl_for_anon_target() {
    EVAL_BXL_FOR_ANON_TARGET.init(
        |dice,
         anon_target: Arc<dyn AnonTargetDyn>,
         global_cfg_options: GlobalCfgOptions,
         dependents_analyses: AnonTargetDependentAnalysisResults<'_>,
         execution_platform,
         liveness| {
            Box::pin(eval_bxl_for_anon_target(
                dice,
                anon_target,
                global_cfg_options,
                dependents_analyses,
                execution_platform,
                liveness,
            ))
        },
    )
}
