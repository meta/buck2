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
use buck2_node::attrs::attr_type::enumeration::EnumAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::typing::Ty;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;
use crate::attrs::coerce::error::CoercionError;

impl AttrTypeCoerce for EnumAttrType {
    fn coerce_item<'v>(
        &self,
        _configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        let s = value.unpack_str_err()?;
        // Enum names in Buck can be specified upper or lower case,
        // so we normalise them to lowercase to make rule implementations easier
        let s = s.to_lowercase();
        if let Some(s) = self.variants.get(s.as_str()) {
            ctx.attr_arena().encode_enum_variant(s)
        } else {
            let wanted = self
                .variants
                .iter()
                .map(|x| x.as_str().to_owned())
                .collect();
            Err(CoercionError::InvalidEnumVariant(s, wanted).into())
        }
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Basic(Ty::string())
    }
}
