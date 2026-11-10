/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_node::configured_universe::CqueryUniverse;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::unconfigured::TargetNode;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use futures::FutureExt;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;

use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::target_list_expr::TargetListExpr;
use crate::bxl::starlark_defs::target_list_expr::TargetListExprArg;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;

#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative
)]
#[derivative(Debug)]
#[display("<universe of {} nodes>", self.target_universe.len())]
pub(crate) struct StarlarkTargetUniverse<'v> {
    // Cquery universe for performing
    target_universe: CqueryUniverse,
    // The target universe as the target set
    target_set: TargetSet<ConfiguredTargetNode>,
    // Trace/Allocative are implemented for BxlContext, but we take a reference here.
    // This is used in unpacking target expressions for lookups.
    #[derivative(Debug = "ignore")]
    #[allocative(skip)]
    ctx: ValueTyped<'v, BxlContext<'v>>,
}

starlark::methods_static!(TARGET_UNIVERSE_METHODS = target_universe_methods);

#[starlark_value(type = "bxl.TargetUniverse", no_freeze)]
impl<'v> StarlarkValue<'v> for StarlarkTargetUniverse<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(TARGET_UNIVERSE_METHODS.methods())
    }
}

impl<'v> StarlarkTargetUniverse<'v> {
    pub(crate) async fn new(
        ctx: ValueTyped<'v, BxlContext<'v>>,
        target_set: TargetSet<ConfiguredTargetNode>,
    ) -> buck2_error::Result<Self> {
        let target_universe = CqueryUniverse::build(&target_set)?;
        let target_set = target_universe
            .get_from_targets(target_set.iter().map(|i| i.label().unconfigured().dupe()));
        Ok(StarlarkTargetUniverse {
            target_universe,
            target_set,
            ctx,
        })
    }
}

/// Target universe in BXL. Used for looking up valid configured targets to use in cquery. This is not needed for uquery.
#[starlark_module]
fn target_universe_methods(builder: &mut MethodsBuilder) {
    /// The target set of the nodes used to construct the target universe.
    fn target_set<'v>(
        this: &'v StarlarkTargetUniverse<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkTargetSet<ConfiguredTargetNode>>> {
        Ok(heap.alloc_representation(StarlarkTargetSet::from(this.target_set.clone())))
    }

    /// The target set of the entire target universe.
    fn universe_target_set<'v>(
        this: &'v StarlarkTargetUniverse<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkTargetSet<ConfiguredTargetNode>>> {
        Ok(heap.alloc_representation(StarlarkTargetSet::from(
            this.target_universe
                .iter()
                .map(|node| node.to_owned())
                .collect::<TargetSet<_>>(),
        )))
    }

    /// Looks up valid configured target nodes within the universe. The targets passed in are either string literals,
    /// unconfigured target nodes, or unconfigured target labels.
    fn lookup<'v>(
        this: &'v StarlarkTargetUniverse<'v>,
        targets: TargetListExprArg<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkTargetSet<ConfiguredTargetNode>>> {
        let heap = eval.heap();
        Ok(this.ctx.via_dice(eval, |dice| {
            dice.via(|dice| {
                async move {
                    let inputs =
                        &*TargetListExpr::<'v, TargetNode>::unpack(targets, &this.ctx, dice)
                            .await?
                            .get(dice)
                            .await?;

                    let result = this
                        .target_universe
                        .get_from_targets(inputs.iter().map(|i| i.label().dupe()));

                    Ok(heap.alloc_representation(StarlarkTargetSet::from(result)))
                }
                .boxed_local()
            })
        })?)
    }
}
