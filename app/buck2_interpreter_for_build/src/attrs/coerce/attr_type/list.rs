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
use either::Either;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::list::StarlarkList;
use starlark::values::tuple::StarlarkTuple;

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
            .items()
            .iter()
            .map(|v| (self.inner).coerce(configurable, ctx, *v))
            .collect::<Result<Vec<_>, _>>()?;
        ctx.attr_arena().encode_list(&items)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::List(Box::new(self.inner.starlark_type()))
    }
}

/// The list or tuple that a list attribute is coerced from.
pub(crate) struct ListOrTuple<'v>(Either<StarlarkList<'v>, StarlarkTuple<'v>>);

impl<'v> ListOrTuple<'v> {
    /// The items. Coercion runs no Starlark code, so a list does not change under the slice.
    pub(crate) fn items(&self) -> &[Value<'v>] {
        match &self.0 {
            Either::Left(list) => list.content(),
            Either::Right(tuple) => tuple.content(),
        }
    }
}

pub(crate) fn coerce_list<'v>(value: Value<'v>) -> buck2_error::Result<ListOrTuple<'v>> {
    if let Some(tuple) = StarlarkTuple::from_value(value) {
        Ok(ListOrTuple(Either::Right(tuple)))
    } else {
        Ok(ListOrTuple(Either::Left(<StarlarkList>::unpack_value_err(
            value,
        )?)))
    }
}
