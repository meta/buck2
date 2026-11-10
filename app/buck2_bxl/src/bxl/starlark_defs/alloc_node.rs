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
}

impl AllocNode for TargetNode {
    fn starlark_ty() -> Ty {
        StarlarkTargetNode::starlark_type_repr()
    }

    fn alloc(self, heap: Heap<'_>) -> Value<'_> {
        heap.alloc_representation(StarlarkTargetNode(self))
            .to_value()
    }
}

impl AllocNode for ConfiguredTargetNode {
    fn starlark_ty() -> Ty {
        StarlarkConfiguredTargetNode::starlark_type_repr()
    }

    fn alloc(self, heap: Heap<'_>) -> Value<'_> {
        heap.alloc_representation(StarlarkConfiguredTargetNode(self))
            .to_value()
    }
}

impl AllocNode for ActionQueryNode {
    fn starlark_ty() -> Ty {
        StarlarkActionQueryNode::starlark_type_repr()
    }

    fn alloc(self, heap: Heap<'_>) -> Value<'_> {
        heap.alloc_representation(StarlarkActionQueryNode(self))
            .to_value()
    }
}
