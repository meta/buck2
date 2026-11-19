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
use buck2_node::attrs::arena::CoercedAttrRef;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::attr_type::AttrTypeInner;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::attrs::configurable::AttrIsConfigurable;
use starlark::values::Value;

use crate::attrs::coerce::AttrTypeCoerce;
use crate::attrs::coerce::attr_type::ty_maybe_select::TyMaybeSelect;
use crate::attrs::coerce::coerced_attr::coerce_attr;

pub mod any;
pub mod arg;
pub mod bool;
pub mod configuration_dep;
pub mod dep;
mod dict;
mod enumeration;
pub mod int;
pub mod label;
mod list;
mod metadata;
mod one_of;
mod option;
pub mod plugin_dep;
pub mod query;
pub mod source;
pub mod split_transition_dep;
mod string;
mod target_modifiers;
mod transition_dep;
mod tuple;
pub(crate) mod ty_maybe_select;
mod visibility;
mod within_view;

pub trait AttrTypeExt {
    fn this(&self) -> &AttrType;

    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        self.this().0.inner.coerce_item(configurable, ctx, value)
    }

    fn coerce<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        self.coerce_with_default(configurable, ctx, value, None)
    }

    fn coerce_with_default<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
        default: Option<CoercedAttrRef<'_>>,
    ) -> buck2_error::Result<AttrToken> {
        coerce_attr(self.this(), configurable, ctx, value, default)
    }

    fn starlark_type(&self) -> TyMaybeSelect {
        self.this().0.inner.starlark_type()
    }
}

impl AttrTypeExt for AttrType {
    fn this(&self) -> &AttrType {
        self
    }
}

pub trait AttrTypeInnerExt {
    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken>;

    fn starlark_type(&self) -> TyMaybeSelect;
}

impl AttrTypeInnerExt for AttrTypeInner {
    fn coerce_item<'v>(
        &self,
        configurable: AttrIsConfigurable,
        ctx: &dyn AttrCoercionContext<'v>,
        value: Value<'v>,
    ) -> buck2_error::Result<AttrToken> {
        match self {
            Self::Any(x) => x.coerce_item(configurable, ctx, value),
            Self::Arg(x) => x.coerce_item(configurable, ctx, value),
            Self::Bool(x) => x.coerce_item(configurable, ctx, value),
            Self::Int(x) => x.coerce_item(configurable, ctx, value),
            Self::Dep(x) => x.coerce_item(configurable, ctx, value),
            Self::Dict(x) => x.coerce_item(configurable, ctx, value),
            Self::List(x) => x.coerce_item(configurable, ctx, value),
            Self::Tuple(x) => x.coerce_item(configurable, ctx, value),
            Self::OneOf(x) => x.coerce_item(configurable, ctx, value),
            Self::Option(x) => x.coerce_item(configurable, ctx, value),
            Self::Source(x) => x.coerce_item(configurable, ctx, value),
            Self::String(x) => x.coerce_item(configurable, ctx, value),
            Self::Query(x) => x.coerce_item(configurable, ctx, value),
            Self::ConfigurationDep(x) => x.coerce_item(configurable, ctx, value),
            Self::ConfiguredDep(x) => x.coerce_item(configurable, ctx, value),
            Self::PluginDep(x) => x.coerce_item(configurable, ctx, value),
            Self::Enum(x) => x.coerce_item(configurable, ctx, value),
            Self::TransitionDep(x) => x.coerce_item(configurable, ctx, value),
            Self::SplitTransitionDep(x) => x.coerce_item(configurable, ctx, value),
            Self::Label(x) => x.coerce_item(configurable, ctx, value),
            Self::Visibility(x) => x.coerce_item(configurable, ctx, value),
            Self::WithinView(x) => x.coerce_item(configurable, ctx, value),
            Self::Metadata(x) => x.coerce_item(configurable, ctx, value),
            Self::TargetModifiers(x) => x.coerce_item(configurable, ctx, value),
        }
    }

    /// Returns a starlark-compatible typing string, e.g. `[str.type]` for values coerced by this
    /// attr.
    fn starlark_type(&self) -> TyMaybeSelect {
        match self {
            AttrTypeInner::Any(x) => x.starlark_type(),
            AttrTypeInner::Arg(x) => x.starlark_type(),
            AttrTypeInner::ConfigurationDep(x) => x.starlark_type(),
            AttrTypeInner::ConfiguredDep(x) => x.starlark_type(),
            AttrTypeInner::Bool(x) => x.starlark_type(),
            AttrTypeInner::Int(x) => x.starlark_type(),
            AttrTypeInner::Dep(x) => x.starlark_type(),
            AttrTypeInner::Dict(x) => x.starlark_type(),
            AttrTypeInner::Enum(x) => x.starlark_type(),
            AttrTypeInner::List(x) => x.starlark_type(),
            AttrTypeInner::Tuple(x) => x.starlark_type(),
            AttrTypeInner::OneOf(x) => x.starlark_type(),
            AttrTypeInner::Option(x) => x.starlark_type(),
            AttrTypeInner::Query(x) => x.starlark_type(),
            AttrTypeInner::PluginDep(x) => x.starlark_type(),
            AttrTypeInner::Source(x) => x.starlark_type(),
            AttrTypeInner::String(x) => x.starlark_type(),
            AttrTypeInner::TransitionDep(x) => x.starlark_type(),
            AttrTypeInner::SplitTransitionDep(x) => x.starlark_type(),
            AttrTypeInner::Label(x) => x.starlark_type(),
            AttrTypeInner::Visibility(x) => x.starlark_type(),
            AttrTypeInner::WithinView(x) => x.starlark_type(),
            AttrTypeInner::Metadata(x) => x.starlark_type(),
            AttrTypeInner::TargetModifiers(x) => x.starlark_type(),
        }
    }
}
