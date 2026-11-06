/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub(crate) mod calculation;
pub(crate) mod registration;

use std::borrow::Borrow;
use std::future::Future;
use std::pin::Pin;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use buck2_common::dice::cells::HasCellResolver;
use buck2_configuration_providers::platform_info::PlatformInfo;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::unsafe_send_future::UnsafeSendFuture;
use buck2_events::dispatch::get_dispatcher;
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::dice::starlark_provider::StarlarkEvalKind;
use buck2_interpreter::factory::BuckStarlarkModule;
use buck2_interpreter::factory::ReentrantStarlarkEvaluator;
use buck2_interpreter::factory::StarlarkEvaluatorProvider;
use buck2_interpreter::print_handler::EventDispatcherPrintHandler;
use buck2_interpreter::soft_error::Buck2StarlarkSoftErrorHandler;
use buck2_node::cfg_constructor::CFG_CONSTRUCTOR_CALCULATION_IMPL;
use buck2_node::cfg_constructor::CfgConstructorImpl;
use buck2_node::metadata::key::MetadataKey;
use buck2_node::metadata::key::MetadataKeyRef;
use buck2_node::modifiers::CfgModifiers;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use buck2_node::nodes::unconfigured::RuleKind;
use buck2_node::rule_type::RuleType;
use calculation::CfgConstructorCalculationInstance;
use dice::DiceComputations;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::values::OwnedFrozen;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;

use crate::registration::init_registration;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum CfgConstructorError {
    #[error(
        "Parameter `refs` to post-constraint analysis function must only contain configuration rules. {0} is not a configuration rule."
    )]
    PostConstraintAnalysisRefsMustBeConfigurationRules(String),
}

#[derive(Allocative, Debug, Pagable)]
pub(crate) struct CfgConstructor {
    pub(crate) cfg_constructor_pre_constraint_analysis: OwnedFrozen<Value<'static>>,
    pub(crate) cfg_constructor_post_constraint_analysis: OwnedFrozen<Value<'static>>,
    pub(crate) key: MetadataKey,
    pub(crate) aliases: Option<OwnedFrozen<Value<'static>>>,
    pub(crate) extra_data: Option<OwnedFrozen<Value<'static>>>,
}

async fn eval_pre_constraint_analysis<'v, 'a>(
    cfg_constructor_pre_constraint_analysis: Value<'v>,
    reentrant_eval: &mut ReentrantStarlarkEvaluator<'v, 'a, '_>,
    cfg: &ConfigurationData,
    package_cfg_modifiers: Option<&CfgModifiers>,
    target_cfg_modifiers: Option<&CfgModifiers>,
    cli_modifiers: &[String],
    rule_type: &RuleType,
    aliases: Option<Value<'v>>,
    extra_data: Option<Value<'v>>,
    configuring_exec_dep: bool,
    print: &'a EventDispatcherPrintHandler,
) -> buck2_error::Result<(Vec<String>, Value<'v>)> {
    reentrant_eval.with_evaluator(|eval| {
        eval.set_print_handler(print);
        eval.set_soft_error_handler(&Buck2StarlarkSoftErrorHandler);

        let legacy_platform = if cfg.is_bound() {
            eval.heap()
                .alloc(PlatformInfo::from_configuration(cfg, eval.heap())?)
        } else {
            Value::new_none()
        };

        let package_cfg_modifiers = eval
            .heap()
            .alloc(NoneOr::from_option(package_cfg_modifiers.copied()));
        let target_cfg_modifiers = eval
            .heap()
            .alloc(NoneOr::from_option(target_cfg_modifiers.copied()));
        let cli_modifiers = eval.heap().alloc(cli_modifiers);
        let rule_name = eval.heap().alloc(rule_type.name());
        let aliases = match aliases {
            Some(v) => v,
            None => Value::new_none(),
        };
        let extra_data = match extra_data {
            Some(v) => v,
            None => Value::new_none(),
        };
        let configuring_exec_dep = eval.heap().alloc(configuring_exec_dep);

        // TODO: should eventually accept cli modifiers and target modifiers (T163570597)
        let pre_constraint_analysis_args = vec![
            ("legacy_platform", legacy_platform),
            ("package_modifiers", package_cfg_modifiers),
            ("target_modifiers", target_cfg_modifiers),
            ("cli_modifiers", cli_modifiers),
            ("rule_name", rule_name),
            ("aliases", aliases),
            ("extra_data", extra_data),
            ("configuring_exec_dep", configuring_exec_dep),
        ];

        // Type check + unpack
        let (refs, params) =
            <(ValueOf<ListOrTupleType<String>>, Value)>::unpack_value_err(eval.eval_function(
                cfg_constructor_pre_constraint_analysis,
                &[],
                &pre_constraint_analysis_args,
            )?)?;

        // `params` Value lives on eval.heap() so we need to move eval out of the closure to keep it alive
        Ok((refs.try_to_vec()?, params))
    })
}

async fn analyze_constraints(
    ctx: &mut DiceComputations<'_>,
    refs: Vec<String>,
) -> buck2_error::Result<BuckMutInsertionMap<String, FrozenProviderCollectionValue>> {
    let cell_resolver = &ctx.get_cell_resolver().await?;
    let cell_alias_resolver = &ctx
        .get_cell_alias_resolver(cell_resolver.root_cell())
        .await?;
    let res = ctx
        .try_compute_join(refs, async |ctx, label_str| {
            // Ensure all refs are configuration rules
            let label = ProvidersLabel::parse(
                &label_str,
                cell_resolver.root_cell(),
                cell_resolver,
                cell_alias_resolver,
            )?;

            if ctx.get_target_node(label.target()).await?.rule_kind() == RuleKind::Configuration {
                Ok((
                    label_str,
                    ctx.get_configuration_analysis_result(&label).await?,
                ))
            } else {
                Err::<_, buck2_error::Error>(
                    CfgConstructorError::PostConstraintAnalysisRefsMustBeConfigurationRules(
                        label_str,
                    )
                    .into(),
                )
            }
        })
        .await?;
    Ok(res.into_iter().collect())
}

fn eval_post_constraint_analysis<'v>(
    cfg_constructor_post_constraint_analysis: Value<'v>,
    params: Value<'v>,
    eval: &mut ReentrantStarlarkEvaluator<'v, '_, '_>,
    refs_providers_map: BuckMutInsertionMap<String, FrozenProviderCollectionValue>,
) -> buck2_error::Result<&'v PlatformInfo<'v>> {
    eval.with_evaluator(|eval| {
        let post_constraint_analysis_args = vec![
            (
                "refs",
                eval.heap().alloc(
                    refs_providers_map
                        .into_iter()
                        .map(|(label, providers)| {
                            (label, providers.add_heap_ref(eval.heap()).to_value())
                        })
                        .collect::<BuckMutInsertionMap<String, Value<'_>>>(),
                ),
            ),
            ("params", params),
        ];

        let post_constraint_analysis_result = eval.eval_function(
            cfg_constructor_post_constraint_analysis,
            &[],
            &post_constraint_analysis_args,
        )?;

        // Type check + unpack
        Ok(<&PlatformInfo>::unpack_value_err(
            post_constraint_analysis_result,
        )?)
    })
}

async fn eval_underlying(
    cfg_constructor: &CfgConstructor,
    ctx: &mut DiceComputations<'_>,
    cfg: &ConfigurationData,
    package_cfg_modifiers: Option<&CfgModifiers>,
    target_cfg_modifiers: Option<&CfgModifiers>,
    cli_modifiers: &[String],
    rule_type: &RuleType,
    configuring_exec_dep: bool,
    cancellation: &CancellationContext,
) -> buck2_error::Result<ConfigurationData> {
    let print = EventDispatcherPrintHandler(get_dispatcher());

    let eval_kind = StarlarkEvalKind::Unknown("constraint-analysis invocation".into());
    let provider = StarlarkEvaluatorProvider::new(ctx, eval_kind).await?;

    BuckStarlarkModule::with_profiling_async(async move |module| {
        let mut reentrant_eval = provider.make_reentrant_evaluator(&module, cancellation.into())?;

        let to_heap = |v: &OwnedFrozen<Value<'static>>| v.as_ref().add_to_heap(module.heap());
        let cfg_constructor_pre_constraint_analysis =
            to_heap(&cfg_constructor.cfg_constructor_pre_constraint_analysis);
        let cfg_constructor_post_constraint_analysis =
            to_heap(&cfg_constructor.cfg_constructor_post_constraint_analysis);
        let aliases = cfg_constructor.aliases.as_ref().map(to_heap);
        let extra_data = cfg_constructor.extra_data.as_ref().map(to_heap);

        // Pre constraint-analysis
        let (refs, params) = eval_pre_constraint_analysis(
            cfg_constructor_pre_constraint_analysis,
            &mut reentrant_eval,
            cfg,
            package_cfg_modifiers,
            target_cfg_modifiers,
            cli_modifiers,
            rule_type,
            aliases,
            extra_data,
            configuring_exec_dep,
            &print,
        )
        .await?;

        // Constraint analysis
        let refs_providers_map = analyze_constraints(ctx, refs).await?;

        // Post constraint-analysis
        let res = eval_post_constraint_analysis(
            cfg_constructor_post_constraint_analysis,
            params,
            &mut reentrant_eval,
            refs_providers_map,
        )?
        .to_configuration(cfg.exec_marking().dupe())?;

        let finished_eval = reentrant_eval.finish_evaluation();
        let (token, _) = finished_eval.finish()?;

        Ok((token, res))
    })
    .await
}

#[pagable_typetag]
#[async_trait]
impl CfgConstructorImpl for CfgConstructor {
    fn eval<'a>(
        &'a self,
        ctx: &'a mut DiceComputations,
        cfg: &'a ConfigurationData,
        package_cfg_modifiers: Option<&'a CfgModifiers>,
        target_cfg_modifiers: Option<&'a CfgModifiers>,
        cli_modifiers: &'a [String],
        rule_type: &'a RuleType,
        configuring_exec_dep: bool,
        cancellation: &'a CancellationContext,
    ) -> Pin<Box<dyn Future<Output = buck2_error::Result<ConfigurationData>> + Send + 'a>> {
        // Get around issue of Evaluator not being send by wrapping future in UnsafeSendFuture
        let fut = async move {
            eval_underlying(
                self,
                ctx,
                cfg,
                package_cfg_modifiers,
                target_cfg_modifiers,
                cli_modifiers,
                rule_type,
                configuring_exec_dep,
                cancellation,
            )
            .await
        };
        unsafe { Box::pin(UnsafeSendFuture::new_encapsulates_starlark(fut)) }
    }

    fn key(&self) -> &MetadataKeyRef {
        self.key.borrow()
    }
}

pub fn init_late_bindings() {
    CFG_CONSTRUCTOR_CALCULATION_IMPL.init(&CfgConstructorCalculationInstance);
    init_registration();
}
