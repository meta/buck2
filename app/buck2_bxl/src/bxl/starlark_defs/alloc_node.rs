/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::actions::query::ActionQueryNode;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::unconfigured::TargetNode;
use dupe::Dupe;
use starlark::typing::Ty;
use starlark::values::Heap;
use starlark::values::StarlarkTypeExpr;
use starlark::values::Value;

use super::nodes::action::StarlarkActionQueryNode;
use crate::bxl::starlark_defs::nodes::configured::StarlarkConfiguredTargetNode;
use crate::bxl::starlark_defs::nodes::unconfigured::StarlarkTargetNode;

/// BXL-specific node allocator.
pub(crate) trait AllocNode {
    /// Starlark type of the values [`AllocNode::alloc`] returns.
    fn starlark_ty() -> Ty;

    fn alloc(self, heap: Heap<'_>) -> Value<'_>;

    /// The values of `nodes`, the nodes of a target set a script is iterating, in order.
    fn alloc_all<'a, 'v>(
        nodes: impl Iterator<Item = &'a Self> + Clone,
        heap: Heap<'v>,
    ) -> Vec<Value<'v>>
    where
        Self: Dupe + 'a,
    {
        nodes.map(|node| node.dupe().alloc(heap)).collect()
    }
}

impl AllocNode for TargetNode {
    fn starlark_ty() -> Ty {
        StarlarkTargetNode::starlark_type_repr()
    }

    fn alloc(self, heap: Heap<'_>) -> Value<'_> {
        heap.alloc(StarlarkTargetNode(self)).to_value()
    }
}

impl AllocNode for ConfiguredTargetNode {
    fn starlark_ty() -> Ty {
        StarlarkConfiguredTargetNode::starlark_type_repr()
    }

    fn alloc(self, heap: Heap<'_>) -> Value<'_> {
        heap.alloc(StarlarkConfiguredTargetNode::new(self))
            .to_value()
    }

    fn alloc_all<'a, 'v>(
        nodes: impl Iterator<Item = &'a Self> + Clone,
        heap: Heap<'v>,
    ) -> Vec<Value<'v>> {
        StarlarkConfiguredTargetNode::all_in_set(nodes)
            .map(|node| heap.alloc_representation(node).to_value())
            .collect()
    }
}

impl AllocNode for ActionQueryNode {
    fn starlark_ty() -> Ty {
        StarlarkActionQueryNode::starlark_type_repr()
    }

    fn alloc(self, heap: Heap<'_>) -> Value<'_> {
        heap.alloc(StarlarkActionQueryNode(self)).to_value()
    }
}
