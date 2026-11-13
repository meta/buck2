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
use buck2_node::attrs::attr_type::one_of::OneOfAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::AttrTypeExt;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;
use crate::attrs::coerce::error::CoercionError;

impl AttrTypeCoerce for OneOfAttrType {
    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        let mut errs = Vec::new();
        // Bias towards the start of the list - try and use success/failure from first in preference
        for (i, x) in self.xs.iter().enumerate() {
            match x.coerce_item(configurable, ctx, value) {
                Ok(v) => return ctx.attr_arena().encode_one_of(i as u32, v),
                // A buck1 `#flavor` label is a label the user meant to write, not a value of some
                // other alternative (`attrs.string()` would accept it as text), so stop here and
                // report the fix instead of the combined failure.
                Err(e) if e.has_tag(buck2_error::ErrorTag::Buck1Flavor) => return Err(e),
                Err(e) => {
                    // TODO(nga): anyhow error creation is expensive.
                    errs.push(e)
                }
            }
        }
        Err(CoercionError::one_of_many(errs))
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Union(
            self.xs
                .iter()
                .map(|x| x.starlark_type())
                .collect::<Vec<_>>(),
        )
    }
}
