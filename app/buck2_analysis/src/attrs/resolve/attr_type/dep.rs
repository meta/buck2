/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use buck2_build_api::interpreter::rule_defs::provider::dependency::Dependency;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_node::attrs::attr_type::configured_dep::ConfiguredExplicitConfiguredDep;
use buck2_node::attrs::attr_type::configured_dep::ExplicitConfiguredDepAttrType;
use buck2_node::attrs::attr_type::dep::DepAttr;
use buck2_node::attrs::attr_type::dep::DepAttrTransition;
use buck2_node::attrs::attr_type::dep::DepAttrType;
use buck2_node::attrs::attr_type::transition_dep::ConfiguredTransitionDep;
use buck2_node::attrs::attr_type::transition_dep::TransitionDepAttrType;
use buck2_node::provider_id_set::ProviderIdSet;
use dupe::Dupe;
use starlark::environment::Module;
use starlark::values::FrozenValueTyped;
use starlark::values::Value;

use crate::attrs::resolve::ctx::AttrResolutionContext;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum ResolutionError {
    #[error(
        "Attribute requires a dep that provides `{0}`, but it was not found on `{1}`. Found these providers: {}",
        .2.join(", "),
)]
    MissingRequiredProvider(String, ConfiguredProvidersLabel, Vec<String>),
}

pub trait DepAttrTypeExt {
    fn check_providers(
        required_providers: &ProviderIdSet,
        providers: &ProviderCollection<'_>,
        target: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<()>;

    fn alloc_dependency<'v>(
        env: &Module<'v>,
        target: &ConfiguredProvidersLabel,
        v: FrozenValueTyped<'v, ProviderCollection<'v>>,
        execution_platform_resolution: Option<&ExecutionPlatformResolution>,
    ) -> Value<'v>;

    fn resolve_single_impl<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        target: &ConfiguredProvidersLabel,
        required_providers: &ProviderIdSet,
        is_exec: bool,
        execution_group: Option<&NamedExecutionGroup>,
    ) -> buck2_error::Result<Value<'v>>;

    fn resolve_single<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        dep_attr: &DepAttr<ConfiguredProvidersLabel>,
    ) -> buck2_error::Result<Value<'v>>;
}

impl DepAttrTypeExt for DepAttrType {
    fn check_providers(
        required_providers: &ProviderIdSet,
        providers: &ProviderCollection<'_>,
        target: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<()> {
        for provider_id in required_providers {
            if !providers.contains_provider(provider_id) {
                return Err(ResolutionError::MissingRequiredProvider(
                    provider_id.name().to_owned(),
                    target.clone(),
                    providers.provider_names(),
                )
                .into());
            }
        }
        Ok(())
    }

    fn alloc_dependency<'v>(
        env: &Module<'v>,
        target: &ConfiguredProvidersLabel,
        v: FrozenValueTyped<'v, ProviderCollection<'v>>,
        execution_platform_resolution: Option<&ExecutionPlatformResolution>,
    ) -> Value<'v> {
        env.heap()
            .alloc_representation(Dependency::new(
                env.heap(),
                target.clone(),
                v.to_value_typed(),
                execution_platform_resolution,
            ))
            .to_value()
    }

    fn resolve_single_impl<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        target: &ConfiguredProvidersLabel,
        required_providers: &ProviderIdSet,
        is_exec_dep: bool,
        execution_group: Option<&NamedExecutionGroup>,
    ) -> buck2_error::Result<Value<'v>> {
        let provider_collection = ctx.get_dep(target)?;
        Self::check_providers(required_providers, provider_collection.as_ref(), target)?;
        let group_resolution = match execution_group {
            Some(group) if is_exec_dep => Some(
                ctx.execution_platform_resolution()
                    .resolution_for_group(&ExecutionGroup::Named(group.dupe()))?,
            ),
            _ => None,
        };
        let execution_platform_resolution = if is_exec_dep {
            Some(
                group_resolution
                    .as_ref()
                    .unwrap_or_else(|| ctx.execution_platform_resolution()),
            )
        } else {
            None
        };

        Ok(Self::alloc_dependency(
            ctx.starlark_module(),
            target,
            provider_collection,
            execution_platform_resolution,
        ))
    }

    fn resolve_single<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        dep_attr: &DepAttr<ConfiguredProvidersLabel>,
    ) -> buck2_error::Result<Value<'v>> {
        let (is_exec, execution_group) = match &dep_attr.attr_type.transition {
            DepAttrTransition::Exec => (true, None),
            DepAttrTransition::ExecInGroup(group) => (true, Some(group)),
            _ => (false, None),
        };
        Self::resolve_single_impl(
            ctx,
            &dep_attr.label,
            &dep_attr.attr_type.required_providers,
            is_exec,
            execution_group,
        )
    }
}

pub(crate) trait ExplicitConfiguredDepAttrTypeExt {
    fn resolve_single<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        dep_attr: &ConfiguredExplicitConfiguredDep,
    ) -> buck2_error::Result<Value<'v>> {
        DepAttrType::resolve_single_impl(
            ctx,
            &dep_attr.label,
            &dep_attr.attr_type.required_providers,
            false,
            None,
        )
    }
}

impl ExplicitConfiguredDepAttrTypeExt for ExplicitConfiguredDepAttrType {}

pub(crate) trait TransitionDepAttrTypeExt {
    fn resolve_single<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        dep_attr: &ConfiguredTransitionDep,
    ) -> buck2_error::Result<Value<'v>> {
        DepAttrType::resolve_single_impl(
            ctx,
            &dep_attr.dep,
            &dep_attr.required_providers,
            false,
            None,
        )
    }
}

impl TransitionDepAttrTypeExt for TransitionDepAttrType {}
