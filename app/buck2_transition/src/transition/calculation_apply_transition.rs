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
use buck2_build_api::actions::query::CONFIGURED_ATTR_TO_VALUE;
use buck2_build_api::actions::query::PackageLabelOption;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_build_api::transition::TRANSITION_CALCULATION;
use buck2_build_api::transition::TransitionCalculation;
use buck2_configuration_providers::platform_info::PlatformInfo;
use buck2_core::configuration::cfg_diff::cfg_diff;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::transition::applied::TransitionApplied;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::provider::label::ProvidersLabel;
use buck2_error::BuckErrorContext;
use buck2_events::dispatch::get_dispatcher;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use buck2_node::attrs::configured_attr::ConfiguredAttr;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use derive_more::Display;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use dupe::OptionDupedExt;
use itertools::Itertools;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::eval::Evaluator;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictType;
use starlark::values::structs::AllocStruct;

use crate::transition::calculation_fetch_transition::FetchTransition;
use crate::transition::calculation_fetch_transition::TransitionData;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
enum ApplyTransitionError {
    #[error("transition function not marked as `split` must return a `PlatformInfo`")]
    NonSplitTransitionMustReturnPlatformInfo,
    #[error("transition function marked `split` must return a dict of `str` to `PlatformInfo`")]
    SplitTransitionMustReturnDict,
    #[error(
        "transition applied again to transition output \
        did not produce identical `PlatformInfo`, the diff:\n{0}"
    )]
    SplitTransitionAgainDifferentPlatformInfo(String),
    #[error(
        "Transition object is not consistent with transition computation params, \
        this may happen because of how DICE recomputation works, \
        a user should never see this message"
    )]
    InconsistentTransitionAndComputation,
}

struct TransitionFunctionArgs<'a, 'v> {
    transition: &'a TransitionData,
    refs: Value<'v>,
    attrs: Option<Value<'v>>,
}

fn call_transition_function<'v>(
    conf: &ConfigurationData,
    params: &TransitionFunctionArgs<'_, 'v>,
    eval: &mut Evaluator<'v, '_, '_>,
) -> buck2_error::Result<TransitionApplied> {
    let TransitionFunctionArgs {
        transition,
        refs,
        attrs,
    } = *params;
    let mut args = vec![(
        "platform",
        eval.heap()
            .alloc(PlatformInfo::from_configuration(conf, eval.heap())?),
    )];
    let impl_ = match transition {
        TransitionData::MagicObject(v) => {
            args.push(("refs", refs));
            v.as_ref().add_to_heap(eval.heap()).as_ref().implementation
        }
        TransitionData::Target(v) => v
            .as_ref()
            .add_to_heap(eval.heap())
            .as_ref()
            .r#impl
            .to_value(),
    };
    if let Some(attrs) = attrs {
        args.push(("attrs", attrs));
    }
    let new_platforms = eval
        .eval_function(impl_, &[], &args)
        .map_err(buck2_error::Error::from)?;
    if transition.is_split() {
        match ValueOf::<DictType<&str, &PlatformInfo>>::new(new_platforms) {
            Some(dict) => {
                let mut split = BuckMutInsertionMap::new();
                for entry in dict.try_items() {
                    let (k, v) = entry?;
                    let prev = split.insert(
                        k.to_owned(),
                        v.to_configuration(conf.exec_marking().dupe())?,
                    );
                    assert!(prev.is_none());
                }
                Ok(TransitionApplied::Split(BuckFixedSortedMap::from(split)))
            }
            None => Err(ApplyTransitionError::SplitTransitionMustReturnDict.into()),
        }
    } else {
        match <&PlatformInfo>::unpack_value_err(new_platforms) {
            Ok(platform) => Ok(TransitionApplied::Single(
                platform.to_configuration(conf.exec_marking().dupe())?,
            )),
            Err(_) => Err(ApplyTransitionError::NonSplitTransitionMustReturnPlatformInfo.into()),
        }
    }
}

async fn do_apply_transition(
    ctx: &mut DiceComputations<'_>,
    attrs: Option<&[Option<Arc<ConfiguredAttr>>]>,
    conf: &ConfigurationData,
    transition_id: &TransitionId,
    cancellation: &CancellationContext,
) -> buck2_error::Result<TransitionApplied> {
    let transition = ctx.fetch_transition(transition_id).await?;
    let mut refs = Vec::new();
    for (s, t) in transition.refs().collect::<Vec<_>>() {
        let provider_collection_value = ctx.fetch_transition_function_reference(t).await?;
        refs.push((s, provider_collection_value));
    }
    let print = EventDispatcherPrintHandler(get_dispatcher());
    let eval_kind = StarlarkEvalKind::Transition(Arc::new(transition_id.clone()));
    let provider = StarlarkEvaluatorProvider::new(ctx, eval_kind).await?;
    BuckStarlarkModule::with_profiling(|module| {
        let (finished_eval, res) =
            provider.with_evaluator(&module, cancellation.into(), |eval, _| {
                eval.set_print_handler(&print);
                eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);
                let refs = module.heap().alloc(AllocStruct(
                    refs.iter()
                        .map(|(s, v)| (*s, v.add_heap_ref(module.heap()))),
                ));
                let attrs = match (transition.attr_names(), attrs) {
                    (Some(names), Some(values)) => {
                        let mut attrs = Vec::new();
                        for (name, value) in names.zip_eq(values.iter()) {
                            let name = name?;
                            let value = match value {
                                Some(value) => (CONFIGURED_ATTR_TO_VALUE.get()?)(
                                    value,
                                    PackageLabelOption::TransitionAttr,
                                    module.heap(),
                                )
                                .with_buck_error_context(|| {
                                    format!(
                                        "Error converting attribute `{}={}` to Starlark value",
                                        name,
                                        value.as_display_no_ctx(),
                                    )
                                })?,
                                None => Value::new_none(),
                            };
                            attrs.push((name, value));
                        }
                        Some(module.heap().alloc(AllocStruct(attrs)))
                    }
                    (None, None) => None,
                    (Some(_), None) | (None, Some(_)) => {
                        return Err(
                            ApplyTransitionError::InconsistentTransitionAndComputation.into()
                        );
                    }
                };
                let params = TransitionFunctionArgs {
                    transition: &transition,
                    refs,
                    attrs,
                };
                match call_transition_function(conf, &params, eval)? {
                    TransitionApplied::Single(new) => {
                        let new_2 = match call_transition_function(&new, &params, eval)
                            .buck_error_context("applying transition again on transition output")?
                        {
                            TransitionApplied::Single(new_2) => new_2,
                            TransitionApplied::Split(_) => {
                                unreachable!(
                                    "split transition filtered out in call_transition_function"
                                )
                            }
                        };
                        if let Err(diff) = cfg_diff(&new, &new_2) {
                            return Err(
                                ApplyTransitionError::SplitTransitionAgainDifferentPlatformInfo(
                                    diff,
                                )
                                .into(),
                            );
                        }
                        Ok(TransitionApplied::Single(new))
                    }
                    TransitionApplied::Split(split) => {
                        // Not validating split transitions yet, because it's not 100% clear what to validate,
                        // and because it is not that important, because split transitions
                        // are not used in per-rule transitions.
                        Ok(TransitionApplied::Split(split))
                    }
                }
            })?;
        let (token, _) = finished_eval.finish()?;
        Ok((token, res))
    })
}

#[async_trait]
pub(crate) trait ApplyTransition {
    /// Resolve `refs` param of transition function.
    async fn fetch_transition_function_reference(
        &mut self,
        target: &ProvidersLabel,
    ) -> buck2_error::Result<FrozenProviderCollectionValue>;
}

#[async_trait]
impl ApplyTransition for DiceComputations<'_> {
    async fn fetch_transition_function_reference(
        &mut self,
        target: &ProvidersLabel,
    ) -> buck2_error::Result<FrozenProviderCollectionValue> {
        Ok(self.get_configuration_analysis_result(target).await?.dupe())
    }
}

struct TransitionCalculationImpl;

pub(crate) fn init_transition_calculation() {
    TRANSITION_CALCULATION.init(&TransitionCalculationImpl);
}

#[async_trait]
impl TransitionCalculation for TransitionCalculationImpl {
    async fn apply_transition(
        &self,
        ctx: &mut DiceComputations<'_>,
        configured_attrs: &BuckMutInsertionMap<&str, Arc<ConfiguredAttr>>,
        cfg: &ConfigurationData,
        transition_id: &TransitionId,
    ) -> buck2_error::Result<Arc<TransitionApplied>> {
        #[derive(Debug, Eq, PartialEq, Hash, Clone, Display, Allocative, Pagable)]
        #[display("{} ({}){}", transition_id, cfg, self.fmt_attrs())]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct TransitionKey {
            cfg: ConfigurationData,
            transition_id: TransitionId,
            /// Attributes which requested by transition function, not all attributes.
            /// The attr value index is the index of attribute in transition object.
            /// Attributes are added here so multiple targets with the equal attributes
            /// (e.g. the same `java_version = 14`) share the transition computation.
            attrs: Option<Vec<Option<Arc<ConfiguredAttr>>>>,
        }

        impl TransitionKey {
            fn fmt_attrs(&self) -> String {
                if let Some(attrs) = &self.attrs {
                    format!(
                        " [{}]",
                        attrs
                            .iter()
                            .map(|a| {
                                if let Some(attr) = a {
                                    attr.as_display_no_ctx().to_string()
                                } else {
                                    "None".to_owned()
                                }
                            })
                            .join(", ")
                    )
                } else {
                    String::new()
                }
            }
        }

        #[async_trait]
        impl Key for TransitionKey {
            type Value = buck2_error::Result<Arc<TransitionApplied>>;

            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                cancellation: &CancellationContext,
            ) -> Self::Value {
                let v: buck2_error::Result<_> = try {
                    do_apply_transition(
                        ctx,
                        self.attrs.as_deref(),
                        &self.cfg,
                        &self.transition_id,
                        cancellation,
                    )
                    .await?
                };

                Ok(Arc::new(v.with_buck_error_context(|| {
                    format!("Error computing transition `{__self}`")
                })?))
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|x, y| {
                    if let (Ok(x), Ok(y)) = (x, y) {
                        x == y
                    } else {
                        false
                    }
                })
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        let transition = ctx.fetch_transition(transition_id).await?;

        let attrs = match transition.attr_names() {
            Some(attrs) => Some(
                attrs
                    .map(|attr| Ok(configured_attrs.get(attr?).duped()))
                    .collect::<buck2_error::Result<_>>()?,
            ),
            None => None,
        };

        let key = TransitionKey {
            cfg: cfg.dupe(),
            transition_id: transition_id.clone(),
            attrs,
        };

        ctx.compute(&key).await?.dupe()
    }
}
