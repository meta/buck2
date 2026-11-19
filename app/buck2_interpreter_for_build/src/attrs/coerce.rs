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
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::values::Value;

use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

pub mod arc_str_interner;
pub mod attr_type;
pub mod coerced_attr;
pub mod ctx;
pub mod error;
pub(crate) mod str_hash;
pub mod testing;

pub trait AttrTypeCoerce {
    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        _ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken>;

    fn starlark_type(&self) -> TyMaybeSelect;
}
