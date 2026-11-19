/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_node::attrs::attr::Attribute;
use buck2_node::attrs::attr_type::AttrType;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::starlark_value;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum StarlarkAttributeError {
    #[error("`attrs.default_only()` cannot be used in nested attributes")]
    DefaultOnlyInNested,
    #[error("`attrs.exec_compatible_with()` cannot be used in nested attributes")]
    ExecCompatibleWithInNested,
    #[error("`attrs.default_only()` cannot wrap `attrs.exec_compatible_with()`")]
    ExecCompatibleWithInDefaultOnly,
}

#[derive(
    derive_more::Display,
    Debug,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{}", attribute)]
pub struct StarlarkAttribute {
    #[starlark_pagable(pagable)]
    attribute: Attribute,
    /// The execution group whose platform the attribute's constraints apply to, for attributes
    /// created by `attrs.exec_compatible_with()`.
    #[starlark_pagable(pagable)]
    exec_compatible_with_group: Option<ExecutionGroup>,
    /// The `ToolMarks` bits of the exec deps, toolchain deps and args within the attribute.
    #[starlark_pagable(pagable)]
    tool_marks: u8,
}

/// How the exec deps, toolchain deps and args within an attribute name their execution group.
/// Attribute types cannot tell an explicit `DefaultExecGroup` from no `exec_group`, so the
/// attribute functions record it here for the checks of rules that declare named execution groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ToolMarks(u8);

impl ToolMarks {
    /// An attribute without exec deps, toolchain deps or args.
    pub(crate) const NONE: ToolMarks = ToolMarks(0);
    pub(crate) const UNMARKED_EXEC_DEP: ToolMarks = ToolMarks(1);
    pub(crate) const UNMARKED_TOOLCHAIN_DEP: ToolMarks = ToolMarks(2);
    pub(crate) const UNMARKED_ARG: ToolMarks = ToolMarks(4);
    /// An `attrs.arg()` with `exec_group = DefaultExecGroup`.
    pub(crate) const DEFAULT_GROUP_ARG: ToolMarks = ToolMarks(8);

    pub(crate) fn union(self, other: ToolMarks) -> ToolMarks {
        ToolMarks(self.0 | other.0)
    }

    pub(crate) fn contains(self, other: ToolMarks) -> bool {
        self.0 & other.0 == other.0
    }

    /// The attribute function of the first exec dep or toolchain dep without `exec_group`.
    pub(crate) fn first_unmarked_dep(self) -> Option<&'static str> {
        [
            (Self::UNMARKED_EXEC_DEP, "attrs.exec_dep()"),
            (Self::UNMARKED_TOOLCHAIN_DEP, "attrs.toolchain_dep()"),
        ]
        .into_iter()
        .find_map(|(mark, function)| self.contains(mark).then_some(function))
    }
}

starlark_simple_value!(StarlarkAttribute);

/// Type of the attribute object returned by methods under [`attrs`](../attrs) namespace, e. g. `attrs.string()`.
#[starlark_module]
fn starlark_attribute_methods(builder: &mut MethodsBuilder) {}

starlark::methods_static!(STARLARK_ATTRIBUTE_METHODS = starlark_attribute_methods);

#[starlark_value(type = "Attr")]
impl<'v> StarlarkValue<'v> for StarlarkAttribute {
    // Used to add type documentation to the generated documentation
    fn get_methods() -> Option<&'static Methods> {
        Some(STARLARK_ATTRIBUTE_METHODS.methods())
    }
}

impl StarlarkAttribute {
    pub(crate) fn new(attr: Attribute, tool_marks: ToolMarks) -> Self {
        Self {
            attribute: attr,
            exec_compatible_with_group: None,
            tool_marks: tool_marks.0,
        }
    }

    pub(crate) fn new_exec_compatible_with(attr: Attribute, group: ExecutionGroup) -> Self {
        Self {
            attribute: attr,
            exec_compatible_with_group: Some(group),
            tool_marks: ToolMarks::NONE.0,
        }
    }

    pub fn attribute(&self) -> &Attribute {
        &self.attribute
    }

    pub(crate) fn tool_marks(&self) -> ToolMarks {
        ToolMarks(self.tool_marks)
    }

    pub fn clone_attribute(&self) -> Attribute {
        self.attribute.clone()
    }

    pub(crate) fn exec_compatible_with_group(&self) -> Option<&ExecutionGroup> {
        self.exec_compatible_with_group.as_ref()
    }

    /// Coercer to put into higher lever coercer (e. g. for `attrs.list(xxx)`).
    pub fn coercer_for_inner(&self) -> buck2_error::Result<AttrType> {
        if self.attribute.is_default_only() {
            return Err(StarlarkAttributeError::DefaultOnlyInNested.into());
        }
        if self.exec_compatible_with_group.is_some() {
            return Err(StarlarkAttributeError::ExecCompatibleWithInNested.into());
        }
        Ok(self.attribute.coercer().dupe())
    }

    pub fn coercer_for_default_only(&self) -> buck2_error::Result<AttrType> {
        if self.exec_compatible_with_group.is_some() {
            return Err(StarlarkAttributeError::ExecCompatibleWithInDefaultOnly.into());
        }
        Ok(self.attribute.coercer().dupe())
    }
}

#[starlark_module]
#[starlark_types(StarlarkAttribute as Attr)]
pub(crate) fn register_attr_type(globals: &mut GlobalsBuilder) {}
