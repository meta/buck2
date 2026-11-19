/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr_type::transition_dep::TransitionDepAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::typing::Ty;
use starlark::values::UnpackValue;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

impl AttrTypeCoerce for TransitionDepAttrType {
    fn coerce_item<'v>(
        &self,
        _configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        let (dep, transition) = if self.transition.is_some() {
            (ctx.coerce_providers_label(value.unpack_str_err()?)?, None)
        } else {
            let (dep, transition) = UnpackValue::unpack_value_err(value)?;
            (
                ctx.coerce_providers_label(dep)?,
                Some(ctx.coerce_providers_label(transition)?),
            )
        };

        ctx.attr_arena()
            .encode_transition_dep(&dep, transition.as_ref())
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Basic(Ty::unions(vec![
            Ty::tuple2(Ty::string(), Ty::string()),
            Ty::string(),
        ]))
    }
}
