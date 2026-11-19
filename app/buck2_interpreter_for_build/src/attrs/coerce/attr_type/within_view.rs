/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_error::internal_error;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr_type::visibility::VisibilityAttrType;
use buck2_node::attrs::attr_type::within_view::WithinViewAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;
use crate::attrs::coerce::attr_type::visibility::parse_visibility_with_view;

impl AttrTypeCoerce for WithinViewAttrType {
    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        if configurable == AttrIsConfigurable::Yes {
            return Err(internal_error!("Within view attribute is not configurable"));
        }
        let v = parse_visibility_with_view(ctx, value)?.build_within_view();
        ctx.attr_arena().encode_within_view(&v)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        // Starlark type of the attribute is the same.
        VisibilityAttrType.starlark_type()
    }
}
