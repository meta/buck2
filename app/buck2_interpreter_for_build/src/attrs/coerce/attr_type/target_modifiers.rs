/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use buck2_interpreter::types::opaque_metadata::OpaqueMetadata;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr_type::target_modifiers::TargetModifiersAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::values::StarlarkTypeExpr;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

impl AttrTypeCoerce for TargetModifiersAttrType {
    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        if configurable == AttrIsConfigurable::Yes {
            return Err(internal_error!("modifiers attribute is not configurable"));
        }
        // Rejects values with no JSON form without building one: the
        // serializer writes into a sink and allocates nothing of its own.
        serde_json::to_writer(std::io::sink(), &value)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
            .with_buck_error_context(|| {
                format!(
                    "Target modifiers attribute is not convertible to JSON: {}",
                    value.to_repr(),
                )
            })?;
        let modifiers = ctx.coerce_cfg_modifiers(value);
        ctx.attr_arena().encode_target_modifiers(&modifiers)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Basic(OpaqueMetadata::starlark_type_repr())
    }
}
