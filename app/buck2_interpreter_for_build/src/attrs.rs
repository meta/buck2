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
use buck2_node::attrs::attr::Attribute;
use buck2_node::attrs::attr::CoercedValue;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::docs::DocString;
use starlark::docs::DocStringKind;
use starlark::values::Value;

use crate::attrs::coerce::attr_type::AttrTypeExt;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;
use crate::attrs::coerce::error::CoercionError;

pub(crate) mod attrs_global;
pub mod coerce;
pub(crate) mod starlark_attribute;
pub(crate) mod visibility_record;
pub use starlark_attribute::StarlarkAttribute;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum AttrCoerceError {
    #[error("Parameter `{0}` had no value provided, but it is mandatory")]
    MissingMandatoryParameter(String),
}

pub trait AttributeCoerceExt {
    fn coerce<'v>(
        &self,
        param_name: &str,
        configurable: AttrIsConfigurable,
        coercer_ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<CoercedValue>;

    fn docstring(&self) -> Option<DocString>;

    fn starlark_type(&self) -> TyMaybeSelect;
}

impl AttributeCoerceExt for Attribute {
    /// Attempt to coerce a value. If the value provided is `None`, and a default value is available,
    /// that default value is returned.
    fn coerce<'v>(
        &self,
        param_name: &str,
        configurable: AttrIsConfigurable,
        coercer_ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<CoercedValue> {
        if self.is_default_only() {
            if value.is_none() {
                return Ok(CoercedValue::Default);
            } else {
                return Err(CoercionError::DefaultOnly(value.to_string()).into());
            }
        }

        match self.default_as_ref() {
            default if !value.is_none() => self
                .coercer()
                .coerce_with_default(configurable, coercer_ctx, value, default)
                .map(CoercedValue::Custom)
                .with_buck_error_context(|| {
                    format!("Error coercing attribute `{param_name}` of type `{self}`")
                }),
            Some(_) => Ok(CoercedValue::Default),
            None => Err(AttrCoerceError::MissingMandatoryParameter(param_name.to_owned()).into()),
        }
    }

    fn docstring(&self) -> Option<DocString> {
        DocString::from_docstring(DocStringKind::Starlark, self.doc())
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        self.coercer().starlark_type()
    }
}
