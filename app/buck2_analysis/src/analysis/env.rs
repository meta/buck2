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

use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::analysis::AnalysisSplitInstants;
use buck2_build_api::analysis::anon_promises_dyn::RunAnonPromisesAccessorPair;
use buck2_build_api::analysis::extra_v::AnalysisExtraValue;
use buck2_build_api::analysis::registry::AnalysisRegistry;
use buck2_build_api::interpreter::rule_defs::cmd_args::value::CommandLineArg;
use buck2_build_api::interpreter::rule_defs::context::AnalysisContext;
use buck2_build_api::interpreter::rule_defs::context::PromisesRun;
use buck2_build_api::interpreter::rule_defs::provider::builtin::template_placeholder_info::TemplatePlaceholderInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::validation_info::ValidationInfo;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValueRef;
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_build_api::validation::transitive_validations::TransitiveValidations;
use buck2_build_api::validation::transitive_validations::TransitiveValidationsData;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::unsafe_send_future::UnsafeSendFuture;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_events::dispatch::get_dispatcher;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use buck2_interpreter::types::rule::FROZEN_PROMISE_ARTIFACT_MAPPINGS_GET_IMPL;
use buck2_interpreter::types::rule::FROZEN_RULE_GET_IMPL;
use buck2_node::nodes::configured::ConfiguredTargetNodeRef;
use buck2_node::rule_type::StarlarkRuleType;
use dice::CancellationContext;
use dice::DiceComputations;
use dupe::Dupe;
use futures::Future;
use starlark::environment::FrozenModule;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::values::FrozenValueTyped;
use starlark::values::Value;
use starlark::values::ValueTyped;

use crate::analysis::plugins::plugins_to_starlark_value;
use crate::attrs::resolve::ctx::AnalysisQueryResult;
use crate::attrs::resolve::ctx::AttrResolutionContext;
use crate::attrs::resolve::node_to_attrs_struct::node_to_attrs_struct;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
enum AnalysisError {
    #[error(
        "Analysis context was missing a query result, this shouldn't be possible. Query was `{0}`"
    )]
    MissingQuery(String),
    #[error("required dependency `{0}` was not found")]
    MissingDep(ConfiguredProvidersLabel),
}

// Contains a `module` that things must live on, and various `FrozenProviderCollectionValue`s
// that are NOT tied to that module. Must claim ownership of them via `add_reference` before returning them.
pub struct RuleAnalysisAttrResolutionContext<'a, 'v> {
    pub module: &'a Module<'v>,
    pub dep_analysis_results: BuckMutMap<ConfiguredTargetLabel, FrozenProviderCollectionValue>,
    pub query_results: BuckMutMap<String, Arc<AnalysisQueryResult>>,
    pub execution_platform_resolution: ExecutionPlatformResolution,
}

impl<'a, 'v> AttrResolutionContext<'v> for &'_ RuleAnalysisAttrResolutionContext<'a, 'v> {
    fn starlark_module(&self) -> &Module<'v> {
        self.module
    }

    fn get_dep(
        &mut self,
        target: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<FrozenValueTyped<'v, ProviderCollection<'v>>> {
        get_dep(&self.dep_analysis_results, target, self.module)
    }

    fn resolve_unkeyed_placeholder(
        &mut self,
        name: &str,
    ) -> buck2_error::Result<Option<CommandLineArg<'v>>> {
        resolve_unkeyed_placeholder(&self.dep_analysis_results, name, self.module)
    }

    fn resolve_query(&mut self, query: &str) -> buck2_error::Result<Arc<AnalysisQueryResult>> {
        resolve_query(&self.query_results, query, self.module)
    }

    fn execution_platform_resolution(&self) -> &ExecutionPlatformResolution {
        &self.execution_platform_resolution
    }
}

pub fn get_dep<'v>(
    dep_analysis_results: &BuckMutMap<ConfiguredTargetLabel, FrozenProviderCollectionValue>,
    target: &ConfiguredProvidersLabel,
    module: &Module<'v>,
) -> buck2_error::Result<FrozenValueTyped<'v, ProviderCollection<'v>>> {
    match dep_analysis_results.get(target.target()) {
        None => Err(AnalysisError::MissingDep(target.dupe()).into()),
        Some(x) => {
            let x = x.lookup_inner(target)?;
            // IMPORTANT: Anything given back to the user must be kept alive
            Ok(x.add_heap_ref(module.heap()))
        }
    }
}

pub fn resolve_unkeyed_placeholder<'v>(
    dep_analysis_results: &BuckMutMap<ConfiguredTargetLabel, FrozenProviderCollectionValue>,
    name: &str,
    module: &Module<'v>,
) -> buck2_error::Result<Option<CommandLineArg<'v>>> {
    // TODO(cjhopman): Make it an error if two deps provide a value for the placeholder.
    for providers in dep_analysis_results.values() {
        let resolved = providers.value.by_ref_with_reconstructor(|collection, r| {
            let Some(placeholder_info) = collection
                .as_ref()
                .builtin_provider::<TemplatePlaceholderInfo>()?
            else {
                return buck2_error::Ok(None);
            };
            let Some(value) = placeholder_info.unkeyed_variables()?.get(name).copied() else {
                return Ok(None);
            };
            // IMPORTANT: Anything given back to the user must be kept alive; the edge
            // makes the dep's heap a dependency of the module's heap.
            Ok(Some(r.edge(module.heap()).rebrand(value)))
        })?;
        if let Some(value) = resolved {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

pub fn resolve_query(
    query_results: &BuckMutMap<String, Arc<AnalysisQueryResult>>,
    query: &str,
    module: &Module,
) -> buck2_error::Result<Arc<AnalysisQueryResult>> {
    match query_results.get(query) {
        None => Err(AnalysisError::MissingQuery(query.to_owned()).into()),
        Some(x) => {
            for (_, y) in x.result.iter() {
                // IMPORTANT: Anything given back to the user must be kept alive
                module.frozen_heap(|fh, _| fh.add_reference(y.as_ref().owner()));
            }
            Ok(x.dupe())
        }
    }
}

pub trait RuleSpec: Sync {
    fn invoke<'v>(
        &self,
        eval: &mut Evaluator<'v, '_, '_>,
        ctx: ValueTyped<'v, AnalysisContext<'v>>,
    ) -> buck2_error::Result<Value<'v>>;

    fn promise_artifact_mappings<'v>(
        &self,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> buck2_error::Result<BuckMutInsertionMap<String, Value<'v>>>;
}

/// Container for the environment that analysis implementation functions should run in
struct AnalysisEnv<'a> {
    rule_spec: &'a dyn RuleSpec,
    deps: Vec<(&'a ConfiguredTargetLabel, AnalysisResult)>,
    query_results: BuckMutMap<String, Arc<AnalysisQueryResult>>,
    execution_platform: &'a ExecutionPlatformResolution,
    label: ConfiguredTargetLabel,
    cancellation: &'a CancellationContext,
}

pub(crate) async fn run_analysis<'a>(
    dice: &'a mut DiceComputations<'_>,
    label: &ConfiguredTargetLabel,
    results: Vec<(&'a ConfiguredTargetLabel, AnalysisResult)>,
    query_results: BuckMutMap<String, Arc<AnalysisQueryResult>>,
    execution_platform: &'a ExecutionPlatformResolution,
    rule_spec: &'a dyn RuleSpec,
    node: ConfiguredTargetNodeRef<'a>,
    cancellation: &CancellationContext,
) -> buck2_error::Result<(AnalysisResult, Option<AnalysisSplitInstants>)> {
    let analysis_env = AnalysisEnv {
        rule_spec,
        deps: results,
        query_results,
        execution_platform,
        label: label.dupe(),
        cancellation,
    };
    run_analysis_with_env(dice, analysis_env, node).await
}

pub fn get_deps_from_analysis_results(
    results: Vec<(&ConfiguredTargetLabel, AnalysisResult)>,
) -> buck2_error::Result<BuckMutMap<ConfiguredTargetLabel, FrozenProviderCollectionValue>> {
    results
        .into_iter()
        .map(|(label, result)| Ok((label.dupe(), result.providers()?.to_owned())))
        .collect::<buck2_error::Result<BuckMutMap<ConfiguredTargetLabel, FrozenProviderCollectionValue>>>()
}

// Used to express that the impl Future below captures multiple named lifetimes.
// See https://github.com/rust-lang/rust/issues/34511#issuecomment-373423999 for more details.
trait Captures<'x> {}
impl<T: ?Sized> Captures<'_> for T {}

fn run_analysis_with_env<'a, 'd: 'a>(
    dice: &'a mut DiceComputations<'d>,
    analysis_env: AnalysisEnv<'a>,
    node: ConfiguredTargetNodeRef<'a>,
) -> impl Future<Output = buck2_error::Result<(AnalysisResult, Option<AnalysisSplitInstants>)>>
+ 'a
+ Captures<'d> {
    let fut = async move { run_analysis_with_env_underlying(dice, analysis_env, node).await };
    unsafe { UnsafeSendFuture::new_encapsulates_starlark(fut) }
}

async fn run_analysis_with_env_underlying(
    dice: &mut DiceComputations<'_>,
    analysis_env: AnalysisEnv<'_>,
    node: ConfiguredTargetNodeRef<'_>,
) -> buck2_error::Result<(AnalysisResult, Option<AnalysisSplitInstants>)> {
    BuckStarlarkModule::with_profiling_async(async move |env| {
        let print = EventDispatcherPrintHandler(get_dispatcher());

        let validations_from_deps = analysis_env
            .deps
            .iter()
            .filter_map(|(label, analysis_result)| {
                analysis_result
                    .validations
                    .dupe()
                    .map(|v| ((*label).dupe(), v))
            })
            .collect::<BuckMutInsertionMap<_, _>>();

        let (attributes, plugins) = {
            let dep_analysis_results = get_deps_from_analysis_results(analysis_env.deps)?;
            let resolution_ctx = RuleAnalysisAttrResolutionContext {
                module: &env,
                dep_analysis_results,
                query_results: analysis_env.query_results,
                execution_platform_resolution: node.execution_platform_resolution().clone(),
            };

            (
                node_to_attrs_struct(node, &mut &resolution_ctx)?,
                plugins_to_starlark_value(node, &mut &resolution_ctx)?,
            )
        };

        let registry = AnalysisRegistry::new_from_owner(
            BaseDeferredKey::TargetLabel(node.label().dupe()),
            analysis_env.execution_platform.dupe(),
        )?;

        let eval_kind = StarlarkEvalKind::Analysis(node.label().dupe());
        let eval_provider = StarlarkEvaluatorProvider::new(dice, eval_kind).await?;
        let mut reentrant_eval =
            eval_provider.make_reentrant_evaluator(&env, analysis_env.cancellation.into())?;

        let (ctx, list_res) = reentrant_eval.with_evaluator(|eval| {
            eval.set_print_handler(&print);
            eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);

            // Test infos that name an execution group record that group's executor at creation,
            // so a test runs with it even when another rule returns the test info.
            if AnalysisExtraValue::get_or_init(eval.module())?
                .execution_platform_resolution
                .set(node.execution_platform_resolution().dupe())
                .is_err()
            {
                return Err(internal_error!("execution platform resolution set twice").into());
            }

            let ctx = AnalysisContext::prepare(
                eval.heap(),
                Some(attributes),
                Some(analysis_env.label),
                Some(plugins.into()),
                registry,
                dice.global_data().get_digest_config(),
            );

            let list_res = analysis_env.rule_spec.invoke(eval, ctx)?;

            Ok((ctx, list_res))
        })?;

        let PromisesRun {
            split_instants,
            requested_anon_targets,
        } = ctx
            .actions
            .run_promises(&mut RunAnonPromisesAccessorPair(&mut reentrant_eval, dice))
            .await?;

        // Pull the ctx object back out, and steal ctx.action's state back
        let analysis_registry = ctx.take_state();

        // TODO: Convert the ValueError from `try_from_value` better than just printing its Debug
        let res_typed = ProviderCollection::try_from_value(list_res)?;
        analysis_registry
            .analysis_value_storage
            .set_result_value(env.heap().alloc_typed(res_typed))?;

        let finished_eval = reentrant_eval.finish_evaluation();

        let declared_actions = analysis_registry.num_declared_actions();
        let declared_artifacts = analysis_registry.num_declared_artifacts();
        let registry_finalizer = analysis_registry.finalize(&env)?;
        let (token, frozen_env, profile_data) = finished_eval.freeze_and_finish(env)?;
        let recorded_values = registry_finalizer(&frozen_env)?;

        let validations = transitive_validations(
            validations_from_deps,
            recorded_values.provider_collection()?,
        )?;

        Ok((
            token,
            (
                AnalysisResult::new(
                    recorded_values,
                    profile_data,
                    BuckFixedSortedMap::default(),
                    declared_actions,
                    declared_artifacts,
                    validations,
                    requested_anon_targets,
                    node.rule_type().dupe(),
                )?,
                split_instants,
            ),
        ))
    })
    .await
}

pub fn transitive_validations(
    deps: BuckMutInsertionMap<ConfiguredTargetLabel, TransitiveValidations>,
    provider_collection: FrozenProviderCollectionValueRef,
) -> buck2_error::Result<Option<TransitiveValidations>> {
    let provider_collection = provider_collection.to_owned();
    let info = provider_collection.builtin_provider_value::<ValidationInfo>()?;
    Ok(if info.is_some() || deps.len() > 1 {
        Some(TransitiveValidations(Arc::new(TransitiveValidationsData {
            info,
            children: deps.into_keys().collect(),
        })))
    } else {
        assert!(
            deps.len() <= 1,
            "Reuse the single element if any from one of the deps for current node."
        );
        deps.into_values().next()
    })
}

pub fn get_rule_callable<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    module: &FrozenModule,
    name: &str,
) -> buck2_error::Result<Value<'v>> {
    let rule_callable = module
        .get_any_visibility(name)
        .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
        .with_buck_error_context(|| format!("Couldn't find rule `{name}`"))?
        .0;
    Ok(rule_callable.add_to_heap(eval.heap()))
}

pub fn get_rule_impl<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    module: &FrozenModule,
    name: &str,
) -> buck2_error::Result<Value<'v>> {
    let rule_callable = get_rule_callable(eval, module, name)?;
    let rule_impl = (FROZEN_RULE_GET_IMPL.get()?)(rule_callable)?;
    Ok(rule_impl)
}

pub fn promise_artifact_mappings<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    module: &FrozenModule,
    name: &str,
) -> buck2_error::Result<BuckMutInsertionMap<String, Value<'v>>> {
    let rule_callable = get_rule_callable(eval, module, name)?;
    let promise_artifact_mappings =
        (FROZEN_PROMISE_ARTIFACT_MAPPINGS_GET_IMPL.get()?)(rule_callable)?;

    Ok(promise_artifact_mappings
        .iter()
        .map(|(name, func)| (name.to_string(), *func))
        .collect::<BuckMutInsertionMap<_, _>>())
}

pub fn get_user_defined_rule_spec(
    module: FrozenModule,
    rule_type: &StarlarkRuleType,
) -> impl RuleSpec + use<> {
    struct Impl {
        module: FrozenModule,
        name: String,
    }

    impl RuleSpec for Impl {
        fn invoke<'v>(
            &self,
            eval: &mut Evaluator<'v, '_, '_>,
            ctx: ValueTyped<'v, AnalysisContext<'v>>,
        ) -> buck2_error::Result<Value<'v>> {
            let rule_impl = get_rule_impl(eval, &self.module, &self.name)?;
            Ok(eval.eval_function(rule_impl, &[ctx.to_value()], &[])?)
        }

        fn promise_artifact_mappings<'v>(
            &self,
            eval: &mut Evaluator<'v, '_, '_>,
        ) -> buck2_error::Result<BuckMutInsertionMap<String, Value<'v>>> {
            promise_artifact_mappings(eval, &self.module, &self.name)
        }
    }

    Impl {
        module,
        name: rule_type.name.clone(),
    }
}
