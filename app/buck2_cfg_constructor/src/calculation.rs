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
use buck2_common::dice::cells::HasCellResolver;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::package::PackageLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::internal_error;
use buck2_interpreter_for_build::interpreter::package_file_calculation::EvalPackageFile;
use buck2_node::cfg_constructor::CfgConstructorCalculationImpl;
use buck2_node::cfg_constructor::CfgConstructorImpl;
use buck2_node::cfg_constructor::CfgConstructorModifiers;
use buck2_node::modifiers::CfgModifiers;
use buck2_node::nodes::unconfigured::TargetNodeRef;
use buck2_node::rule_type::RuleType;
use buck2_node::super_package::SuperPackage;
use derive_more::Display;
use dice::CancellationContext;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use dupe::OptionDupedExt;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum CalculationCfgConstructorError {
    #[error(
        "Target `{0}` sets `metadata[\"buck.cfg_modifiers\"]` which is no longer supported. \
         Use the first-class `modifiers` attribute instead."
    )]
    MetadataModifiersNotSupported(TargetLabel),
}

pub struct CfgConstructorCalculationInstance;

async fn get_cfg_constructor_uncached(
    ctx: &mut DiceComputations<'_>,
) -> buck2_error::Result<Option<Arc<dyn CfgConstructorImpl>>> {
    let root_cell = ctx.get_cell_resolver().await?.root_cell();
    let package_label = PackageLabel::new(root_cell, CellRelativePath::empty())?;
    // This returns empty super package if `PACKAGE` file does not exist.
    let super_package = ctx.eval_package_file(package_label).await?;
    Ok(super_package.cfg_constructor().duped())
}

async fn get_cfg_constructor<'d>(
    ctx: &mut DiceComputations<'d>,
) -> buck2_error::Result<&'d Option<Arc<dyn CfgConstructorImpl>>> {
    #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct GetCfgConstructorKey;

    #[async_trait]
    impl Key for GetCfgConstructorKey {
        type Value = buck2_error::Result<Option<Arc<dyn CfgConstructorImpl>>>;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            get_cfg_constructor_uncached(ctx).await
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::AlwaysUnequal
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            OkPagableValueSerialize::<Self::Value>::new()
        }
    }

    ctx.compute(&GetCfgConstructorKey)
        .await?
        .as_ref()
        .duped_err()
}

#[async_trait]
impl CfgConstructorCalculationImpl for CfgConstructorCalculationInstance {
    async fn eval_cfg_constructor(
        &self,
        ctx: &mut DiceComputations<'_>,
        target: TargetNodeRef<'_>,
        super_package: &SuperPackage,
        cfg: ConfigurationData,
        modifiers: CfgConstructorModifiers,
        rule_type: &RuleType,
    ) -> buck2_error::Result<ConfigurationData> {
        #[derive(Clone, Display, Dupe, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
        #[display("CfgConstructorInvocationKey")]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct CfgConstructorInvocationKey {
            package_cfg_modifiers: Option<CfgModifiers>,
            target_cfg_modifiers: Option<CfgModifiers>,
            cfg: ConfigurationData,
            modifiers: CfgConstructorModifiers,
            rule_type: RuleType,
        }

        #[async_trait]
        impl Key for CfgConstructorInvocationKey {
            type Value = buck2_error::Result<ConfigurationData>;

            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                cancellation: &CancellationContext,
            ) -> Self::Value {
                let cfg_constructor =
                    get_cfg_constructor(ctx).await?.as_ref().ok_or_else(|| {
                        internal_error!("Global cfg constructor instance should exist")
                    })?;
                let highest_priority_modifiers = self.modifiers.render(&self.cfg)?;
                // TODO(jtbraun) drive these errors to zero and turn these into errors at PACKAGE/BUCK load time
                if let Some(m) = &self.package_cfg_modifiers {
                    m.report_unresolvable();
                }
                if let Some(m) = &self.target_cfg_modifiers {
                    m.report_unresolvable();
                }
                cfg_constructor
                    .eval(
                        ctx,
                        &self.cfg,
                        self.package_cfg_modifiers.as_ref(),
                        self.target_cfg_modifiers.as_ref(),
                        &highest_priority_modifiers,
                        &self.rule_type,
                        self.modifiers.configuring_exec_dep(),
                        cancellation,
                    )
                    .await
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|x, y| match (x, y) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                })
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        let Some(cfg_constructor) = get_cfg_constructor(ctx).await? else {
            // To facilitate rollout of modifiers, return original configuration if
            // no cfg constructors are available.
            return Ok(cfg);
        };
        let modifier_key = cfg_constructor.key();
        let package_cfg_modifiers = super_package.cfg_modifiers().duped();

        // metadata["buck.cfg_modifiers"] is no longer supported. Fail loudly so the developer
        // knows the modifier they wrote won't be applied.
        if target
            .metadata()?
            .is_some_and(|m| m.get(modifier_key).is_some())
        {
            return Err(
                CalculationCfgConstructorError::MetadataModifiersNotSupported(
                    target.label().dupe(),
                )
                .into(),
            );
        }

        let target_cfg_modifiers = target.target_modifiers()?.filter(|t| !t.is_empty());

        // If there are no PACKAGE/target/cli modifiers, return the original configuration without computing DICE call
        // TODO(scottcao): This is just for rollout purpose. Remove once modifier is rolled out
        if package_cfg_modifiers.is_none()
            && target_cfg_modifiers.is_none()
            && modifiers.is_empty(&cfg)?
        {
            return Ok(cfg);
        }

        let key = CfgConstructorInvocationKey {
            package_cfg_modifiers,
            target_cfg_modifiers,
            cfg,
            modifiers,
            rule_type: rule_type.dupe(),
        };
        Ok(ctx.compute(&key).await?.dupe()?)
    }
}
