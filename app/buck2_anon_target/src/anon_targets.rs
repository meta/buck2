/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::mem;
use std::sync::Arc;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_analysis::analysis::calculation::get_loaded_module;
use buck2_analysis::analysis::env::RuleSpec;
use buck2_analysis::analysis::env::get_rule_callable;
use buck2_analysis::analysis::env::get_user_defined_rule_spec;
use buck2_analysis::analysis::env::transitive_validations;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::analysis::AnalysisKeyActivationData;
use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::analysis::AnalysisSplitInstants;
use buck2_build_api::analysis::AnalysisWithExtraData;
use buck2_build_api::analysis::AnonTargetSplitData;
use buck2_build_api::analysis::anon_promises_dyn::AnonPromisesDyn;
use buck2_build_api::analysis::anon_promises_dyn::RunAnonPromisesAccessorPair;
use buck2_build_api::analysis::anon_targets_registry::ANON_TARGET_REGISTRY_NEW;
use buck2_build_api::analysis::anon_targets_registry::AnonTargetsRegistryDyn;
use buck2_build_api::analysis::registry::AnalysisRegistry;
use buck2_build_api::anon_target::AnonTargetDependentAnalysisResults;
use buck2_build_api::anon_target::AnonTargetDyn;
use buck2_build_api::artifact_groups::promise::PromiseArtifact;
use buck2_build_api::artifact_groups::promise::PromiseArtifactId;
use buck2_build_api::artifact_groups::promise::PromiseArtifactResolveError;
use buck2_build_api::build::detailed_aggregated_metrics::dice::HasDetailedAggregatedMetrics;
use buck2_build_api::deferred::calculation::DeferredHolder;
use buck2_build_api::deferred::calculation::EVAL_ANON_TARGET;
use buck2_build_api::deferred::calculation::GET_PROMISED_ARTIFACT;
use buck2_build_api::interpreter::rule_defs::context::AnalysisContext;
use buck2_build_api::interpreter::rule_defs::context::PromisesRun;
use buck2_build_api::interpreter::rule_defs::plugins::AnalysisPlugins;
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_build_signals::env::WaitingData;
use buck2_build_signals::node_key::BuildSignalsNodeKey;
use buck2_build_signals::node_key::BuildSignalsNodeKeyImpl;
use buck2_configured::execution::find_execution_platform_by_configuration;
use buck2_core::cells::name::CellName;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::base_deferred_key::BaseDeferredKeyDyn;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::execution::ExecutionPlatformResolutionPartial;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::package::PackageLabel;
use buck2_core::pattern::pattern::PatternData;
use buck2_core::pattern::pattern::lex_target_pattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::target::label::label::TargetLabel;
use buck2_core::target::name::TargetNameRef;
use buck2_core::unsafe_send_future::UnsafeSendFuture;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_events::dispatch::async_record_root_spans;
use buck2_events::dispatch::get_dispatcher;
use buck2_events::dispatch::span_async;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use buck2_interpreter::starlark_promise::StarlarkPromise;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_interpreter_for_build::attrs::coerce::arc_str_interner::ArcStrInterner;
use buck2_interpreter_for_build::rule::FrozenStarlarkRuleCallable;
use buck2_interpreter_for_build::rule::frozen_rule_attribute_spec;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::spec::internal::is_internal_attr;
use buck2_node::bzl_or_bxl_path::BzlOrBxlPath;
use buck2_node::rule_type::RuleType;
use buck2_node::rule_type::StarlarkRuleType;
use buck2_util::arc_str::ArcStr;
use buck2_util::time_span::TimeSpan;
use buck2_util::vec2::Vec2;
use derive_more::Display;
use dice::Demand;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use futures::FutureExt;
use futures::future::BoxFuture;
use pagable::Pagable;
use starlark::any::AnyLifetime;
use starlark::any::ProvidesStaticType;
use starlark::codemap::FileSpan;
use starlark::values::DynStarlark;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;

use crate::anon_promises::AnonPromises;
use crate::anon_target_attr::AnonTargetAttr;
use crate::anon_target_attr_coerce::AnonTargetAttrTypeCoerce;
use crate::anon_target_attr_resolve::AnonTargetDependents;
use crate::anon_target_node::AnonAttrValues;
use crate::anon_target_node::AnonTarget;
use crate::anon_target_node::AnonTargetVariant;
use crate::bxl::eval_bxl_for_anon_target;
use crate::promise_artifacts::PromiseArtifactRegistry;

#[derive(Debug, Trace, Allocative, ProvidesStaticType)]
pub struct AnonTargetsRegistry<'v> {
    // We inherit the execution platform of our parent
    execution_platform: ExecutionPlatformResolution,
    promises: AnonPromises<'v>,
    promise_artifact_registry: PromiseArtifactRegistry,
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum AnonTargetsError {
    #[error("Not allowed to call `anon_targets` in this context")]
    AssertNoPromisesFailed,
    #[error("Invalid `name` attribute, must be a label or a string, got `{value}` of type `{typ}`")]
    InvalidNameType { typ: String, value: String },
    #[error("`name` attribute must be a valid target label, got `{0}`")]
    InvalidTargetLabel(String),
    #[error("Unknown attribute `{0}`")]
    UnknownAttribute(String),
    #[error("Internal attribute `{0}` not allowed as argument to `anon_targets`")]
    InternalAttribute(String),
    #[error("Missing attribute `{0}`")]
    MissingAttribute(String),
    #[error(
        "Rule `{0}` declares execution groups, which anon targets do not have. Use a rule without execution groups, and pass `exec_group` to `anon_target()` to run it on the platform of one of the caller's groups"
    )]
    RuleWithExecutionGroups(String),
    #[error(
        "Anon target of rule `{rule}` is created in execution group `{group}`, which has no compatible execution platform. Run `buck2 audit execution-platform-resolution` on the target to see why each platform was skipped"
    )]
    NoExecutionPlatformForGroup { rule: String, group: String },
    #[error(
        "The anon target uses the default execution group, which the rule does not declare. Pass `exec_group`, or add `DefaultExecGroup` to `exec_groups`"
    )]
    DefaultExecutionGroupNotDeclared,
    #[error("Query macros are not supported")]
    QueryMacroNotSupported,
}

use pagable::pagable_typetag;

#[derive(
    Hash, Eq, PartialEq, Clone, Dupe, Debug, Display, Trace, Allocative, Pagable
)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub(crate) struct AnonTargetKey(pub(crate) Arc<AnonTarget>);

#[async_trait]
impl Key for AnonTargetKey {
    type Value = buck2_error::Result<AnalysisResult>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellation: &CancellationContext,
    ) -> Self::Value {
        let deferred_key = DeferredHolderKey::Base(BaseDeferredKey::AnonTarget(self.0.dupe()));
        ctx.analysis_started(&deferred_key)?;

        let analysis_start = Instant::now();
        let (res, split_instants, spans) = {
            let (r, spans) = async_record_root_spans(self.run_analysis(ctx, cancellation)).await;
            let (result, split) = r?;
            (result, split, spans)
        };
        let analysis_end = Instant::now();

        let (time_span, anon_target_split) = match split_instants {
            Some(split) => (
                TimeSpan::new_saturating(analysis_start, split.pre_promises),
                Some(AnonTargetSplitData {
                    part2_time_span: TimeSpan::new_saturating(split.post_promises, analysis_end),
                }),
            ),
            None => (TimeSpan::new_saturating(analysis_start, analysis_end), None),
        };

        ctx.store_evaluation_data(AnalysisKeyActivationData {
            waiting_data: WaitingData::new(),
            time_span,
            spans,
            analysis_with_extra_data: AnalysisWithExtraData {
                target_rule_type_name: None,
            },
            anon_target_split,
        })?;

        ctx.analysis_complete(&deferred_key, &DeferredHolder::Analysis(&res))?;
        Ok(res)
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::AlwaysUnequal
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }

    fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
        demand.provide_value_with(|| BuildSignalsNodeKey::new(self.dupe()));
    }
}

impl BuildSignalsNodeKeyImpl for AnonTargetKey {
    fn critical_path_entry_proto(&self) -> Option<buck2_data::critical_path_entry2::Entry> {
        Some(
            buck2_data::critical_path_entry2::AnonAnalysis {
                anon_target: Some(self.0.as_proto()),
                part: None,
            }
            .into(),
        )
    }

    fn kind(&self) -> &'static str {
        "anon_target"
    }
}

impl AnonTargetKey {
    pub(crate) fn downcast(key: Arc<dyn BaseDeferredKeyDyn>) -> buck2_error::Result<Self> {
        Ok(AnonTargetKey(
            key.into_any()
                .downcast()
                .ok()
                .internal_error("Expecting AnonTarget")?,
        ))
    }

    fn prepare_anon_target_data<'v>(
        execution_platform: &ExecutionPlatformResolution,
        uses_execution_groups: bool,
        rule: ValueTyped<'v, FrozenStarlarkRuleCallable<'v>>,
        attributes: ValueOf<'v, DictType<&'v str, Value<'v>>>,
    ) -> buck2_error::Result<(
        Arc<StarlarkRuleType>,
        TargetLabel,
        AnonAttrValues,
        ConfigurationNoExec,
    )> {
        if rule.declares_execution_groups() {
            return Err(
                AnonTargetsError::RuleWithExecutionGroups(rule.rule_type().name.clone()).into(),
            );
        }

        let mut name = None;

        let attrs_spec = rule.attributes();
        let anon_attr_ctx = AnonAttrCtx::new(execution_platform, uses_execution_groups);

        let mut provided: BuckMutInsertionMap<&str, Value> =
            BuckMutInsertionMap::with_capacity(attributes.dict().len());
        for entry in attributes.try_items() {
            let (k, v) = entry?;
            if k == "name" {
                name = Some(Self::coerce_name(v)?);
            } else if is_internal_attr(k) {
                return Err(AnonTargetsError::InternalAttribute(k.to_owned()).into());
            } else if attrs_spec.attribute(k).is_none() {
                return Err(AnonTargetsError::UnknownAttribute(k.to_owned()).into());
            } else {
                provided.insert(k, v);
            }
        }

        // Internal attrs are never stored (rejected above or skipped here),
        // and every non-internal attr gets a value (provided or default), so
        // the allocation is sized exactly; it is retained for the lifetime
        // of the anon target key.
        let mut attr_values = Vec2::with_capacity(attrs_spec.num_non_internal_attrs());
        for (k, id, a) in attrs_spec.attr_specs() {
            if is_internal_attr(k) {
                continue;
            }
            let value = match provided.get(k) {
                Some(v) => Self::coerce_to_anon_target_attr(a.coercer(), *v, &anon_attr_ctx)
                    .with_buck_error_context(|| format!("Error coercing attribute `{k}`"))?,
                None => match a.default_as_ref() {
                    Some(x) => AnonTargetAttr::from_attr_ref(k, x, a.coercer())?,
                    None => return Err(AnonTargetsError::MissingAttribute(k.to_owned()).into()),
                },
            };
            attr_values.push(id, value);
        }
        let attrs = AnonAttrValues::new(attr_values);

        // We need to ensure there is a "name" attribute which corresponds to something we can turn in to a label.
        // If there isn't a good one, make something up
        let name = match name {
            None => Self::create_name(&rule.rule_type().name)?,
            Some(name) => name,
        };
        Ok((
            rule.rule_type().dupe(),
            name,
            attrs,
            execution_platform.base_cfg().dupe(),
        ))
    }

    /// `uses_execution_groups` is whether the creator declares named execution groups or creates
    /// the anon target with `exec_group`.
    pub(crate) fn new<'v>(
        execution_platform: &ExecutionPlatformResolution,
        uses_execution_groups: bool,
        rule: ValueTyped<'v, FrozenStarlarkRuleCallable<'v>>,
        attributes: ValueOf<'v, DictType<&'v str, Value<'v>>>,
        owner_key: &BaseDeferredKey,
    ) -> buck2_error::Result<Self> {
        let (rule_type, name, attrs, exec_cfg) = Self::prepare_anon_target_data(
            execution_platform,
            uses_execution_groups,
            rule,
            attributes,
        )?;

        let global_cfg_options = match owner_key {
            BaseDeferredKey::TargetLabel(_) => None,
            BaseDeferredKey::AnonTarget(anon) => anon.global_cfg_options(),
            BaseDeferredKey::BxlLabel(bxl) => bxl.0.global_cfg_options(),
        };
        let anon_target = match (&rule_type.path, global_cfg_options) {
            (BzlOrBxlPath::Bzl(_), _) => {
                AnonTarget::new(rule_type, name, attrs, exec_cfg, AnonTargetVariant::Bzl)
            }
            (BzlOrBxlPath::Bxl(_), Some(global_cfg_options)) => AnonTarget::new(
                rule_type,
                name,
                attrs,
                exec_cfg,
                AnonTargetVariant::Bxl(global_cfg_options),
            ),
            (BzlOrBxlPath::Bxl(bxl_file_path), None) => {
                return Err(internal_error!(
                    "Bxl anon target defined at {} must have global configuration options.",
                    bxl_file_path
                ));
            }
        };

        Ok(Self(Arc::new(anon_target)))
    }

    /// We need to parse a TargetLabel from a String, but it doesn't matter if the pieces aren't
    /// valid targets in the context of this build (e.g. if the package really exists),
    /// just that it is syntactically valid.
    fn parse_target_label(x: &str) -> buck2_error::Result<TargetLabel> {
        let err = || {
            format!(
                "`name` attribute must be a valid target label, got `{}`",
                x.to_owned()
            )
        };
        let lex =
            lex_target_pattern::<TargetPatternExtra>(x, false).with_buck_error_context(err)?;
        // TODO(nga): `CellName` contract requires it refers to declared cell name.
        //   This `unchecked_new` violates it.
        let cell =
            CellName::unchecked_new(lex.cell_alias.filter(|a| !a.is_empty()).unwrap_or("anon"))?;
        match lex.pattern.reject_ambiguity()? {
            PatternData::TargetInPackage {
                package,
                target_name,
                extra: TargetPatternExtra,
                modifiers: _,
            } => Ok(TargetLabel::new(
                PackageLabel::new(
                    cell,
                    CellRelativePath::new(<&ForwardRelativePath>::try_from(package)?),
                )?,
                target_name.as_ref(),
            )),
            _ => Err(AnonTargetsError::InvalidTargetLabel(x.to_owned()).into()),
        }
    }

    fn create_name(rule_name: &str) -> buck2_error::Result<TargetLabel> {
        // TODO(nga): this creates non-existing cell reference.
        let cell_name = CellName::unchecked_new("anon")?;
        let pkg = PackageLabel::new(cell_name, CellRelativePath::empty())?;
        Ok(TargetLabel::new(pkg, TargetNameRef::new(rule_name)?))
    }

    fn coerce_name(x: Value) -> buck2_error::Result<TargetLabel> {
        if let Some(x) = StarlarkConfiguredProvidersLabel::from_value(x) {
            Ok(x.label().target().unconfigured().dupe())
        } else if let Some(x) = x.unpack_str() {
            Self::parse_target_label(x)
        } else {
            Err(AnonTargetsError::InvalidNameType {
                typ: x.get_type().to_owned(),
                value: x.to_string(),
            }
            .into())
        }
    }

    fn coerce_to_anon_target_attr(
        attr: &AttrType,
        x: Value,
        ctx: &AnonAttrCtx,
    ) -> buck2_error::Result<AnonTargetAttr> {
        attr.coerce_item(ctx, x)
    }

    pub(crate) async fn resolve<'d>(
        &self,
        dice: &mut DiceComputations<'d>,
    ) -> buck2_error::Result<&'d AnalysisResult> {
        dice.compute(self).await?.as_ref().duped_err()
    }

    /// Like [`Self::resolve`], also returning the key allocation DICE holds
    /// for this anon target, shared by all its requesters.
    pub(crate) async fn resolve_with_key<'d>(
        &self,
        dice: &mut DiceComputations<'d>,
    ) -> buck2_error::Result<(Arc<AnonTargetKey>, &'d AnalysisResult)> {
        let (key, result) = dice.compute_with_key(self).await?;
        Ok((key, result.as_ref().duped_err()?))
    }

    fn run_analysis<'a>(
        &'a self,
        dice: &'a mut DiceComputations<'_>,
        cancellation: &'a CancellationContext,
    ) -> BoxFuture<'a, buck2_error::Result<(AnalysisResult, Option<AnalysisSplitInstants>)>> {
        let fut = async move { self.run_analysis_impl(dice, cancellation).await };
        Box::pin(unsafe { UnsafeSendFuture::new_encapsulates_starlark(fut) })
    }

    async fn run_analysis_impl(
        &self,
        dice: &mut DiceComputations<'_>,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<(AnalysisResult, Option<AnalysisSplitInstants>)> {
        let dependents = AnonTargetDependents::get_dependents(self)?;
        let dependents_analyses = dependents.get_analysis_results(dice).await?;

        let exec_resolution = ExecutionPlatformResolutionPartial::new(
            Some(
                find_execution_platform_by_configuration(
                    dice,
                    self.0.exec_cfg().cfg(),
                    self.0.exec_cfg().cfg(),
                )
                .await?,
            ),
            Vec::new(),
        )
        .finalize(BuckFixedSortedMap::new());

        span_async(
            buck2_data::AnalysisStart {
                target: Some(self.0.as_proto().into()),
                rule: self.0.rule_type().to_string(),
            },
            async move {
                let res = match (&self.0.rule_type().path, self.0.anon_target_type()) {
                    (BzlOrBxlPath::Bxl(_), AnonTargetVariant::Bxl(global_cfg_options)) => {
                        cancellation
                            .with_structured_cancellation(|observer| {
                                eval_bxl_for_anon_target(
                                    dice,
                                    self.0.dupe(),
                                    global_cfg_options.dupe(),
                                    dependents_analyses,
                                    exec_resolution,
                                    observer,
                                )
                                .boxed_local()
                            })
                            .await
                            .map(|r| (r, None))
                    }
                    (BzlOrBxlPath::Bzl(_), AnonTargetVariant::Bzl) => {
                        self.eval_for_bzl(dice, dependents_analyses, exec_resolution, cancellation)
                            .await
                    }
                    (BzlOrBxlPath::Bxl(bxl_file_path), AnonTargetVariant::Bzl) => {
                        Err(internal_error!(
                            "Bxl anon target defined at {}, but AnonTarget key is bzl.",
                            bxl_file_path
                        ))
                    }
                    (BzlOrBxlPath::Bzl(import_path), AnonTargetVariant::Bxl(_)) => {
                        Err(internal_error!(
                            "Bzl anon target defined at {}, but AnonTarget key is bxl.",
                            import_path
                        ))
                    }
                };
                let end = buck2_data::AnalysisEnd {
                    target: Some(self.0.as_proto().into()),
                    rule: self.0.rule_type().to_string(),
                    profile: None, // Not implemented for anon targets
                    declared_actions: res.as_ref().ok().map(|(v, _)| v.num_declared_actions),
                    declared_artifacts: res.as_ref().ok().map(|(v, _)| v.num_declared_artifacts),
                    error: res.as_ref().err().map(|e| format!("{e:#}")),
                };
                (res, end)
            },
        )
        .await
    }

    async fn eval_for_bzl(
        &self,
        dice: &mut DiceComputations<'_>,
        dependents_analyses: AnonTargetDependentAnalysisResults<'_>,
        exec_resolution: ExecutionPlatformResolution,
        cancellation: &CancellationContext,
    ) -> buck2_error::Result<(AnalysisResult, Option<AnalysisSplitInstants>)> {
        let validations_from_deps = dependents_analyses.validations();
        let module = get_loaded_module(dice, self.0.rule_type()).await?;
        let rule_impl = get_user_defined_rule_spec(module.env().dupe(), self.0.rule_type());

        let eval_kind = self.0.dupe().eval_kind();
        let provider = StarlarkEvaluatorProvider::new(dice, eval_kind).await?;

        BuckStarlarkModule::with_profiling_async(async move |env| {
            let print = EventDispatcherPrintHandler(get_dispatcher());
            let mut reentrant_eval =
                provider.make_reentrant_evaluator(&env, cancellation.into())?;
            let (ctx, list_res) = reentrant_eval.with_evaluator(|eval| {
                eval.set_print_handler(&print);
                eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);

                let rule_callable =
                    get_rule_callable(eval, module.env(), &self.0.rule_type().name)?;
                let attrs_spec = frozen_rule_attribute_spec(rule_callable)?;
                let attributes = self.0.resolve_attrs(
                    &env,
                    attrs_spec,
                    dependents_analyses,
                    exec_resolution.clone(),
                )?;

                let registry = AnalysisRegistry::new_from_owner(
                    BaseDeferredKey::AnonTarget(self.0.dupe()),
                    exec_resolution,
                )?;

                let ctx = AnalysisContext::prepare(
                    eval.heap(),
                    Some(attributes),
                    Some(self.0.configured_label()),
                    // FIXME(JakobDegen): There should probably be a way to pass plugins
                    // into anon targets
                    Some(
                        eval.heap()
                            .alloc_typed(AnalysisPlugins::new(BuckMutInsertionMap::new()))
                            .into(),
                    ),
                    registry,
                    dice.global_data().get_digest_config(),
                );

                let list_res = rule_impl.invoke(eval, ctx)?;
                Ok((ctx, list_res))
            })?;

            let PromisesRun {
                split_instants,
                requested_anon_targets,
            } = ctx
                .actions
                .run_promises(&mut RunAnonPromisesAccessorPair(&mut reentrant_eval, dice))
                .await?;

            let res_typed = ProviderCollection::try_from_value(list_res)?;
            let res = env.heap().alloc(res_typed);

            let fulfilled_artifact_mappings = reentrant_eval.with_evaluator(|eval| {
                let promise_artifact_mappings = rule_impl.promise_artifact_mappings(eval)?;

                self.0
                    .dupe()
                    .get_fulfilled_promise_artifacts(promise_artifact_mappings, res, eval)
            })?;

            let res =
                ValueTyped::new(res).internal_error("Just allocated the provider collection")?;

            // Pull the ctx object back out, and steal ctx.action's state back
            let analysis_registry = ctx.take_state();
            analysis_registry
                .analysis_value_storage
                .set_result_value(res)?;
            let finished_eval = reentrant_eval.finish_evaluation();
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
                (
                    AnalysisResult::new(
                        recorded_values,
                        None,
                        fulfilled_artifact_mappings,
                        num_declared_actions,
                        num_declared_artifacts,
                        validations,
                        requested_anon_targets,
                        RuleType::Starlark(self.0.rule_type().dupe()),
                    )?,
                    split_instants,
                ),
            ))
        })
        .await
    }
}

/// Several attribute functions need a context, make one that is mostly useless.
pub(crate) struct AnonAttrCtx {
    pub(crate) execution_platform_resolution: ExecutionPlatformResolution,
    /// Whether the creator declares named execution groups or creates the anon target with
    /// `exec_group`.
    pub(crate) uses_execution_groups: bool,
    str_interner: ArcStrInterner,
}

impl AnonAttrCtx {
    fn new(
        execution_platform_resolution: &ExecutionPlatformResolution,
        uses_execution_groups: bool,
    ) -> Self {
        Self {
            execution_platform_resolution: execution_platform_resolution.clone(),
            uses_execution_groups,
            str_interner: ArcStrInterner::new(),
        }
    }

    pub(crate) fn intern_str(&self, value: &str) -> ArcStr {
        self.str_interner.intern(value)
    }
}

pub(crate) fn init_eval_anon_target() {
    EVAL_ANON_TARGET
        .init(|ctx, key| Box::pin(async move { AnonTargetKey::downcast(key)?.resolve(ctx).await }));
}

pub(crate) fn init_get_promised_artifact() {
    GET_PROMISED_ARTIFACT.init(|promise_artifact, ctx| {
        Box::pin(
            async move { get_artifact_from_anon_target_analysis(promise_artifact.id(), ctx).await },
        )
    });
}

pub(crate) async fn get_artifact_from_anon_target_analysis<'d>(
    promise_id: &PromiseArtifactId,
    ctx: &mut DiceComputations<'d>,
) -> buck2_error::Result<&'d Artifact> {
    let owner = promise_id.owner();
    let analysis_result = match owner {
        BaseDeferredKey::AnonTarget(anon_target) => {
            AnonTargetKey::downcast(anon_target.dupe())?
                .resolve(ctx)
                .await?
        }
        _ => {
            return Err(PromiseArtifactResolveError::OwnerIsNotAnonTarget(
                promise_id.clone(),
                owner.clone(),
            )
            .into());
        }
    };

    Ok(analysis_result
        .promise_artifact_map()
        .get(promise_id)
        .ok_or_else(|| PromiseArtifactResolveError::NotFoundInAnalysis(promise_id.clone()))?)
}

pub(crate) fn init_anon_target_registry_new() {
    ANON_TARGET_REGISTRY_NEW.init(|_phantom, execution_platform| {
        Box::new(DynStarlark::new(AnonTargetsRegistry {
            execution_platform,
            promises: AnonPromises::default(),
            promise_artifact_registry: PromiseArtifactRegistry::new(),
        }))
    });
}

impl<'v> AnonTargetsRegistry<'v> {
    pub(crate) fn downcast_mut<'a>(
        registry: &'a mut dyn AnonTargetsRegistryDyn<'v>,
    ) -> buck2_error::Result<&'a mut AnonTargetsRegistry<'v>> {
        registry
            .as_any_mut()
            .downcast_mut::<AnonTargetsRegistry<'v>>()
            .ok_or_else(|| internal_error!("AnonTargetsRegistryDyn is not an AnonTargetsRegistry"))
    }

    pub(crate) fn anon_target_key(
        &self,
        rule: ValueTyped<'v, FrozenStarlarkRuleCallable<'v>>,
        attributes: ValueOf<'v, DictType<&'v str, Value<'v>>>,
        owner_key: &BaseDeferredKey,
        execution_group: Option<&ExecutionGroup>,
    ) -> buck2_error::Result<AnonTargetKey> {
        if matches!(execution_group, None | Some(ExecutionGroup::Default))
            && !self.execution_platform.default_execution_group_declared()
        {
            return Err(AnonTargetsError::DefaultExecutionGroupNotDeclared.into());
        }
        match execution_group {
            None => AnonTargetKey::new(
                &self.execution_platform,
                self.execution_platform.has_execution_groups(),
                rule,
                attributes,
                owner_key,
            ),
            Some(group) => {
                // Fails for an unspecified resolution, which resolves no group, as registering an
                // action in any group does.
                if let ExecutionGroup::Named(named) = group
                    && self.execution_platform.platform_for_group(group)?.is_none()
                {
                    return Err(AnonTargetsError::NoExecutionPlatformForGroup {
                        rule: rule.rule_type().name.clone(),
                        group: named.to_string(),
                    }
                    .into());
                }
                AnonTargetKey::new(
                    &self.execution_platform.resolution_for_group(group)?,
                    true,
                    rule,
                    attributes,
                    owner_key,
                )
            }
        }
    }

    pub(crate) fn register_one(
        &mut self,
        promise: ValueTyped<'v, StarlarkPromise<'v>>,
        key: AnonTargetKey,
    ) -> buck2_error::Result<()> {
        self.promises.push_one(promise, key);

        Ok(())
    }

    pub(crate) fn register_artifact(
        &mut self,
        location: Option<FileSpan>,
        anon_target_key: AnonTargetKey,
        id: usize,
    ) -> buck2_error::Result<PromiseArtifact> {
        let anon_target_key = BaseDeferredKey::AnonTarget(anon_target_key.0.dupe());
        let id = PromiseArtifactId::new(anon_target_key, id);
        self.promise_artifact_registry.register(location, id)
    }
}

impl<'v> AnonTargetsRegistryDyn<'v> for AnonTargetsRegistry<'v> {
    fn as_any_mut(&mut self) -> &mut dyn AnyLifetime<'v> {
        self
    }

    fn consumer_analysis_artifacts(&self) -> Vec<PromiseArtifact> {
        self.promise_artifact_registry.consumer_analysis_artifacts()
    }

    fn take_promises(&mut self) -> Option<Box<dyn AnonPromisesDyn<'v>>> {
        // We swap it out, so we can still collect new promises
        Some(mem::take(&mut self.promises))
            .filter(|p| !p.is_empty())
            .map(|p| Box::new(p) as Box<dyn AnonPromisesDyn>)
    }

    /*
    pub(crate) fn get_promises(&mut self) -> Option<AnonTargetsRegistry<'v>> {
        if self.entries.is_empty() {
            None
        } else {
            // We swap it out, so we can still collect new promises
            let mut new = AnonTargetsRegistry::new(self.execution_platform.dupe());
            mem::swap(&mut new, self);
            Some(new)
        }
    }
    */

    fn assert_no_promises(&self) -> buck2_error::Result<()> {
        if self.promises.is_empty() {
            Ok(())
        } else {
            Err(AnonTargetsError::AssertNoPromisesFailed.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anon_target_name() {
        assert_eq!(
            AnonTargetKey::parse_target_label("//foo:bar")
                .unwrap()
                .to_string(),
            "anon//foo:bar"
        );
        assert_eq!(
            AnonTargetKey::parse_target_label("cell//foo/bar:baz")
                .unwrap()
                .to_string(),
            "cell//foo/bar:baz"
        );
        assert!(AnonTargetKey::parse_target_label("foo").is_err());
        assert!(AnonTargetKey::parse_target_label("//foo:").is_err());
    }
}
