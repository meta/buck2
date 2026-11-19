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
use buck2_node::attrs::attr_type::list::ListAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::list::ListRef;
use starlark::values::tuple::TupleRef;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::AttrTypeExt;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

impl AttrTypeCoerce for ListAttrType {
    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        let list = coerce_list(value)?;
        let items = list
            .iter()
            .map(|v| (self.inner).coerce(configurable, ctx, *v))
            .collect::<Result<Vec<_>, _>>()?;
        ctx.attr_arena().encode_list(&items)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::List(Box::new(self.inner.starlark_type()))
    }
}

pub(crate) fn coerce_list<'v>(value: Value<'v>) -> buck2_error::Result<&'v [Value<'v>]> {
    if let Some(list) = TupleRef::from_value(value) {
        Ok(list.content())
    } else {
        Ok(<&ListRef>::unpack_value_err(value)?.content())
    }
}
