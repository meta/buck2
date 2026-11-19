/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::package::PackageLabel;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;

use crate::attrs::arena::CoercedAttrRef;
use crate::attrs::attr::Attribute;
use crate::attrs::configuration_context::AttrConfigurationContext;
use crate::attrs::configured_attr_full::ConfiguredAttrFull;
use crate::attrs::configured_traversal::ConfiguredAttrTraversal;
use crate::attrs::traversal::CoercedAttrTraversal;

/// Full coerced attribute: name, type, value.
pub struct CoercedAttrFull<'a> {
    pub name: &'a str,
    pub attr: &'a Attribute,
    pub value: CoercedAttrRef<'a>,
}

impl<'a> CoercedAttrFull<'a> {
    pub fn configure(
        &self,
        ctx: &dyn AttrConfigurationContext,
    ) -> ResultMaybeCompatible<ConfiguredAttrFull<'a>> {
        self.value
            .configure(self.attr.coercer(), ctx, Some(self.name))
            .map(|v| ConfiguredAttrFull {
                name: self.name,
                attr: self.attr,
                value: v,
            })
            .tag(ErrorTag::ConfigureAttr)
            .with_buck_error_context(|| format!("configuring attr `{}`", self.name))
    }

    pub fn traverse(
        &self,
        pkg: PackageLabel,
        traversal: &mut dyn CoercedAttrTraversal<'a>,
    ) -> buck2_error::Result<()> {
        self.value
            .traverse(self.attr.coercer(), Some(pkg), traversal)
            .with_buck_error_context(|| format!("traversing attribute `{}`", self.name))
    }

    /// Visit the deps this attr's configured form would contain, without
    /// materializing the configured value.
    pub fn traverse_configured(
        &self,
        pkg: PackageLabel,
        ctx: &dyn AttrConfigurationContext,
        traversal: &mut dyn ConfiguredAttrTraversal,
    ) -> ResultMaybeCompatible<()> {
        self.value
            .traverse_configured(self.attr.coercer(), pkg, ctx, traversal, Some(self.name))
            .tag(ErrorTag::ConfigureAttr)
            .with_buck_error_context(|| format!("configuring attr `{}`", self.name))
    }
}
