/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::provider::label::ProvidersLabel;
use buck2_node::attrs::attr_type::configuration_dep::ConfigurationDepAttrType;
use starlark::values::Value;

use crate::attrs::resolve::ctx::AttrResolutionContext;

pub(crate) trait ConfigurationDepAttrTypeExt {
    fn resolve_single<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        label: &ProvidersLabel,
    ) -> buck2_error::Result<Value<'v>> {
        Ok(ctx.heap().alloc(label.to_string()).to_value())
    }
}

impl ConfigurationDepAttrTypeExt for ConfigurationDepAttrType {}
