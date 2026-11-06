/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::nodes::configured::ConfiguredTargetNodeRef;
use starlark::values::ValueOf;
use starlark::values::structs::AllocStruct;
use starlark::values::structs::StructRef;

use crate::attrs::resolve::configured_attr::ConfiguredAttrExt;
use crate::attrs::resolve::ctx::AttrResolutionContext;

/// Prepare `ctx.attrs` for rule impl.
pub(crate) fn node_to_attrs_struct<'v>(
    node: ConfiguredTargetNodeRef,
    ctx: &mut dyn AttrResolutionContext<'v>,
) -> buck2_error::Result<ValueOf<'v, StructRef<'v>>> {
    let attrs_iter = node.attrs(AttrInspectOptions::All);
    let mut resolved_attrs = Vec::with_capacity(attrs_iter.len());
    for a in attrs_iter {
        resolved_attrs.push((a.name, a.value.resolve_single(node.label().pkg(), ctx)?));
    }
    Ok(ValueOf::alloc_struct(
        ctx.heap(),
        AllocStruct(resolved_attrs),
    ))
}
