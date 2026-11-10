/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::plugins::AnalysisPlugins;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_hash::BuckMutInsertionMap;
use buck2_node::attrs::attr_type::dep::DepAttr;
use buck2_node::attrs::attr_type::dep::DepAttrTransition;
use buck2_node::attrs::attr_type::dep::DepAttrType;
use buck2_node::nodes::configured::ConfiguredTargetNodeRef;
use buck2_node::provider_id_set::ProviderIdSet;
use dupe::IterDupedExt;
use starlark::values::Value;
use starlark::values::ValueTyped;

use crate::attrs::resolve::attr_type::dep::DepAttrTypeExt;
use crate::attrs::resolve::ctx::AttrResolutionContext;

pub fn plugins_to_starlark_value<'v>(
    node: ConfiguredTargetNodeRef,
    ctx: &mut dyn AttrResolutionContext<'v>,
) -> buck2_error::Result<ValueTyped<'v, AnalysisPlugins<'v>>> {
    let mut plugins = BuckMutInsertionMap::new();
    let execution_platform_resolution = node.execution_platform_resolution();

    for kind in node.uses_plugins().iter().duped() {
        let deps: Vec<Value<'_>> = node
            .plugin_lists()
            .iter_for_kind(&kind)
            .map(|(target, _)| {
                // Get the correct cfg for this plugin target from exec_dep_cfgs
                let cfg = execution_platform_resolution.cfg_for_exec_dep(target)?;
                let configured_target = target.configure(cfg);

                DepAttrType::resolve_single(
                    ctx,
                    &DepAttr {
                        attr_type: DepAttrType::new(ProviderIdSet::EMPTY, DepAttrTransition::Exec),
                        label: ConfiguredProvidersLabel::default_for(configured_target),
                    },
                )
            })
            .collect::<buck2_error::Result<_>>()?;
        plugins.insert(kind, ctx.heap().alloc(deps));
    }
    Ok(ctx
        .heap()
        .alloc_representation(AnalysisPlugins::new(plugins)))
}
