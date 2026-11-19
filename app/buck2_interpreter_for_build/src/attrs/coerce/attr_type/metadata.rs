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
use buck2_hash::BuckMutInsertionMap;
use buck2_interpreter::types::opaque_metadata::OpaqueMetadata;
use buck2_node::attrs::arena::AttrToken;
use buck2_node::attrs::attr_type::metadata::MetadataAttrType;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use buck2_node::metadata::key::MetadataKeyRef;
use buck2_node::metadata::map::MetadataMap;
use buck2_node::metadata::value::MetadataValue;
use starlark::values::StarlarkTypeExpr;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::dict::DictRef;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;

impl AttrTypeCoerce for MetadataAttrType {
    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        if configurable == AttrIsConfigurable::Yes {
            return Err(internal_error!("Metadata attribute is not configurable"));
        }

        let dict = DictRef::unpack_value_err(value)?;

        let mut map = BuckMutInsertionMap::with_capacity(dict.len());
        for (key, value) in dict.iter() {
            let key = MetadataKeyRef::new(key.unpack_str_err()?)?;

            let value = value
                .to_json_value()
                .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
                .with_buck_error_context(|| {
                    format!(
                        "Metadata attribute with key {} is not convertible to JSON: {}",
                        key.to_owned(),
                        value.to_repr(),
                    )
                })?;

            map.insert(key.to_owned(), MetadataValue::new(value));
        }

        let m = MetadataMap::new(map);
        ctx.attr_arena().encode_metadata(&m)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        TyMaybeSelect::Basic(OpaqueMetadata::starlark_type_repr())
    }
}
