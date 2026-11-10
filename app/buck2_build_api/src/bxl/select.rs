/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;

use allocative::Allocative;
use buck2_core::package::PackageLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_error::internal_error;
use buck2_interpreter::types::configured_providers_label::StarlarkProvidersLabel;
use buck2_node::attrs::arena::CoercedAttrRef;
use buck2_node::attrs::arena::OwnedCoercedAttr;
use buck2_node::attrs::arena::Refs;
use buck2_node::attrs::arena::Values;
use buck2_node::attrs::coerced::CoercedAttrRefKind;
use buck2_node::attrs::coerced::select::AttrRefSelectorEntries;
use buck2_node::attrs::coerced::select::CoercedSelectorKeyRef;
use buck2_node::attrs::display::AttrDisplayWithContext;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::serialize::AttrSerializeWithContext;
use derivative::Derivative;
use dupe::Dupe;
use either::Either;
use serde::Serialize;
use starlark::__derive_refs::serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::const_frozen_string;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::bxl::unconfigured_attribute::ref_to_value;

type SelectDictKey<'v> = Either<ValueOf<'v, StarlarkProvidersLabel>, StringValue<'v>>;

#[derive(
    ProvidesStaticType,
    Derivative,
    Trace,
    Allocative,
    Clone,
    Debug,
    StarlarkPagable
)]
pub struct StarlarkSelectDict {
    /// Handle to a `Selector` value in its owning storage.
    #[starlark_pagable(pagable)]
    selector: OwnedCoercedAttr,
    #[starlark_pagable(pagable)]
    pkg: PackageLabel,
}

impl Display for StarlarkSelectDict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ctx = AttrFmtContext {
            package: Some(self.pkg),
            options: Default::default(),
        };
        self.selector.as_ref().fmt(&ctx, f)
    }
}

impl Serialize for StarlarkSelectDict {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let ctx = AttrFmtContext {
            package: Some(self.pkg),
            options: Default::default(),
        };
        self.selector.as_ref().serialize_with_ctx(&ctx, serializer)
    }
}

#[derive(UnpackValue, derive_more::Display, Clone, Dupe)]
enum SelectDictKeyArg<'v> {
    #[display("{}", _0)]
    Label(ValueOf<'v, StarlarkProvidersLabel>),
    #[display("{}", _0)]
    Str(&'v str),
}

impl StarlarkSelectDict {
    /// `selector` must reference a `Selector` value.
    pub fn new(selector: OwnedCoercedAttr, pkg: PackageLabel) -> Self {
        Self { selector, pkg }
    }

    fn entries(
        &self,
    ) -> buck2_error::Result<(AttrRefSelectorEntries<'_>, Option<CoercedAttrRef<'_>>)> {
        match self.selector.as_ref().kind() {
            CoercedAttrRefKind::Selector(entries_and_default) => Ok(entries_and_default),
            _ => Err(internal_error!(
                "SelectDict handle must reference a selector value"
            )),
        }
    }

    fn get<'v>(
        &self,
        key: SelectDictKeyArg<'v>,
        heap: Heap<'v>,
    ) -> buck2_error::Result<NoneOr<Value<'v>>> {
        let (entries, default) = self.entries()?;
        let found = match key {
            SelectDictKeyArg::Label(label) => entries
                .iter()
                .find(|(k, _)| *k == label.label())
                .map(|(_, v)| v),
            SelectDictKeyArg::Str(key) => {
                if key == CoercedSelectorKeyRef::DEFAULT_KEY_STR {
                    default
                } else {
                    entries
                        .iter()
                        .find(|(k, _)| k.to_string() == key)
                        .map(|(_, v)| v)
                }
            }
        };
        match found {
            Some(v) => Ok(NoneOr::Other(ref_to_value(
                v,
                &self.selector,
                self.pkg,
                heap,
            )?)),
            None => Ok(NoneOr::None),
        }
    }
}

starlark_simple_value!(StarlarkSelectDict);

starlark::methods_static!(SELECT_DICT_METHODS = select_dict_methods);

#[starlark_value(type = "bxl.SelectDict")]
impl<'v> StarlarkValue<'v> for StarlarkSelectDict {
    fn get_methods() -> Option<&'static Methods> {
        Some(SELECT_DICT_METHODS.methods())
    }
}

#[inline(always)]
fn target_key<'v>(label: &ProvidersLabel, heap: Heap<'v>) -> SelectDictKey<'v> {
    Either::Left(ValueOf::alloc(
        heap,
        StarlarkProvidersLabel::new(label.dupe()),
    ))
}

#[inline(always)]
fn default_key<'v>() -> SelectDictKey<'v> {
    Either::Right(const_frozen_string!(CoercedSelectorKeyRef::DEFAULT_KEY_STR).at())
}

/// In bxl, `Select = bxl.SelectDict | bxl.SelectConcat`. `bxl.SelectDict` is a dict-like object that represents a select.
/// One example of this type is:
///
/// ```python
/// select({
///      "root//constraints:a": ["--foo"],
///      "root//constraints:b": ["--bar"],
///      "DEFAULT": ["baz"]
///  })
/// ```
///
/// You can:
///
/// * Iterate over its keys (e.g., `for key in select_dict.select_keys():`).
/// * Iterate over key-value pairs using select_dict.select_items() (e.g., `for key, value in select_dict.select_items():`).
/// * Get the select entry with a string or a ProvidersLabel (e.g., `select_dict.get_select_entry("root//constraints:a")`).
/// * Check its type using `isinstance(select_dict, bxl.SelectDict)``.
#[starlark_module]
fn select_dict_methods(builder: &mut MethodsBuilder) {
    /// Return the key-value pairs of the select. The key is either a string (for `DEFAULT`) or a `ProvidersLabel`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_select_dict(ctx):
    ///     node = ctx.lazy.unconfigured_target_node("root//:select_dict").resolve()
    ///     attr = node.get_attr("select_attr")
    ///     for key, value in attr.select_items():
    ///         ctx.output.print(f"{key} -> {value}")
    /// ```
    fn select_items<'v>(
        this: &'v StarlarkSelectDict,
        heap: Heap<'v>,
    ) -> starlark::Result<Vec<(SelectDictKey<'v>, Value<'v>)>> {
        let (entries, default) = this.entries()?;
        let mut items: Vec<(SelectDictKey, Value)> = entries
            .iter()
            .map(|(k, v)| {
                ref_to_value(v, &this.selector, this.pkg.dupe(), heap)
                    .map(|v| (target_key(k, heap), v))
            })
            .collect::<buck2_error::Result<_>>()?;
        if let Some(d) = default {
            items.push((
                default_key(),
                ref_to_value(d, &this.selector, this.pkg.dupe(), heap)?,
            ));
        }
        Ok(items)
    }

    /// Return the keys of SelectDict. The key is either a string (for `DEFAULT`) or a `ProvidersLabel`.
    ///
    /// Sample usage:
    ///
    /// ```python
    /// def _impl_select_dict(ctx):
    ///     node = ctx.lazy.unconfigured_target_node("root//:select_dict").resolve()
    ///     attr = node.get_attr("select_attr")
    ///     for key in attr.select_keys():
    ///         ctx.output.print(key)
    /// ```
    fn select_keys<'v>(
        this: &'v StarlarkSelectDict,
        heap: Heap<'v>,
    ) -> starlark::Result<Vec<SelectDictKey<'v>>> {
        let (entries, default) = this.entries()?;
        let mut keys: Vec<SelectDictKey> =
            entries.iter().map(|(k, _)| target_key(k, heap)).collect();
        if default.is_some() {
            keys.push(default_key());
        }
        Ok(keys)
    }

    /// Return the entry of the select for the given key. It accepts either a string or a `ProvidersLabel`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_select_dict(ctx):
    ///    node = ctx.lazy.unconfigured_target_node("root//:select_dict").resolve()
    ///    attr = node.get_attr("select_attr")
    ///    ctx.output.print(attr.get_select_entry("root//constraints:a"))
    ///    ctx.output.print(attr.get_select_entry("DEFAULT"))
    ///    # provider_label's type here is `ProvidersLabel`
    ///    ctx.output.print(attr.get_select_entry(provider_label))
    /// ```
    fn get_select_entry<'v>(
        this: &'v StarlarkSelectDict,
        #[starlark(require = pos)] key: SelectDictKeyArg<'v>,
        heap: Heap<'v>,
    ) -> starlark::Result<NoneOr<Value<'v>>> {
        Ok(this.get(key, heap)?)
    }
}

#[derive(
    ProvidesStaticType,
    Derivative,
    Trace,
    Allocative,
    Clone,
    Debug,
    StarlarkPagable
)]
pub struct StarlarkSelectConcat {
    /// Handle to a `Concat` value in its owning storage.
    #[starlark_pagable(pagable)]
    concat: OwnedCoercedAttr,
    #[starlark_pagable(pagable)]
    pkg: PackageLabel,
}

impl StarlarkSelectConcat {
    /// `concat` must reference a `Concat` value.
    pub fn new(concat: OwnedCoercedAttr, pkg: PackageLabel) -> Self {
        Self { concat, pkg }
    }

    fn items(&self) -> buck2_error::Result<Refs<'_, Values>> {
        match self.concat.as_ref().kind() {
            CoercedAttrRefKind::Concat(items) => Ok(items),
            _ => Err(internal_error!(
                "SelectConcat handle must reference a concat value"
            )),
        }
    }
}

impl Serialize for StarlarkSelectConcat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let ctx = AttrFmtContext {
            package: Some(self.pkg),
            options: Default::default(),
        };
        self.concat.as_ref().serialize_with_ctx(&ctx, serializer)
    }
}

impl Display for StarlarkSelectConcat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ctx = AttrFmtContext {
            package: Some(self.pkg),
            options: Default::default(),
        };
        self.concat.as_ref().fmt(&ctx, f)
    }
}

starlark_simple_value!(StarlarkSelectConcat);

starlark::methods_static!(SELECT_CONCAT_METHODS = select_concat_methods);

#[starlark_value(type = "bxl.SelectConcat")]
impl<'v> StarlarkValue<'v> for StarlarkSelectConcat {
    fn get_methods() -> Option<&'static Methods> {
        Some(SELECT_CONCAT_METHODS.methods())
    }

    fn length(&self) -> starlark::Result<i32> {
        Ok(self.items()?.len() as i32)
    }
}

/// In bxl, `Select = bxl.SelectDict | bxl.SelectConcat`. `bxl.SelectConcat` is a list-like object that represents a select.
/// One example of this type is:
///
/// ```python
/// ["--flags"] + select({
///     "root//constraints:a": ["--foo"],
///     "root//constraints:b": ["--bar"],
///     "DEFAULT": ["baz"]
/// })
/// ```
///
/// You can:
///
/// * Iterate over the values of this object (e.g. `for item in select_concat.select_iter():`)
/// * Get the length (e.g. `len(select_concat)`)
/// * Check its type using `isinstance(select_concat, bxl.SelectConcat)`.
///
/// Sample usage:
/// ```python
/// def _impl_select_concat(ctx):
///     node = ctx.lazy.unconfigured_target_node("root//:select_concat").resolve()
///     attr = node.get_attr("select_attr")
///     for value in attr:
///         if isinstance(value, bxl.SelectDict):
///             for key, value in value.items():
///                 ctx.output.print(f"{key} -> {value}")
///         else:
///             ctx.output.print(value)
///     ctx.output.print(attr[0])
/// ```
#[starlark_module]
fn select_concat_methods(builder: &mut MethodsBuilder) {
    /// Return the values of the SelectConcat.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_select_concat(ctx):
    ///     node = ctx.lazy.unconfigured_target_node("root//:select_concat").resolve()
    ///     attr = node.get_attr("select_attr")
    ///     for value in attr.select_iter():
    ///         ctx.output.print(value)
    /// ```
    fn select_iter<'v>(
        this: &'v StarlarkSelectConcat,
        heap: Heap<'v>,
    ) -> starlark::Result<Vec<Value<'v>>> {
        let list = this
            .items()?
            .iter()
            .map(|a| ref_to_value(a, &this.concat, this.pkg.dupe(), heap))
            .collect::<buck2_error::Result<_>>()?;
        Ok(list)
    }

    /// Returns the length of a SelectConcat, defined as the number of items being concatenated
    /// at the select level (not the total number of elements across all lists).
    ///
    /// For example, `[1, 2] + select({"DEFAULT": [3, 4]}` returns 2 instead of 4.
    /// Note: You can use `len()` to get the length too.
    #[starlark(attribute)]
    fn length(this: &StarlarkSelectConcat) -> starlark::Result<i32> {
        Ok(this.items()?.len() as i32)
    }
}

#[starlark_module]
#[starlark_types(
    StarlarkSelectDict as SelectDict,
    StarlarkSelectConcat as SelectConcat
)]
pub fn register_select_types(globals: &mut GlobalsBuilder) {}
