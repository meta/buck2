/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;

use allocative::Allocative;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use dupe::Dupe;
use pagable::Pagable;
use starlark::eval::Evaluator;
use starlark::starlark_simple_value;
use starlark::typing::Ty;
use starlark::values::AllocValue;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkRepresentations;
use starlark::values::StarlarkType;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;
use starlark::values::type_repr::StarlarkTypeRepr;

#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    Pagable,
    starlark::StarlarkPagableViaPagable
)]
enum InnerStarlarkExecutionGroup {
    #[display("<execution_group <unbound>>")]
    Unbound(CellPath),
    #[display("<execution_group {_0}>")]
    Bound(NamedExecutionGroup),
}

/// An execution group, created via `exec_group()`.
///
/// A rule declares the execution groups it uses with `rule(exec_groups = [...])`. Exec deps
/// and toolchain deps declared with `exec_group = ...`, and actions run with
/// `ctx.actions.run(..., exec_group = ...)`, use the execution platform resolved for that
/// group instead of the target's execution platform.
#[derive(
    derive_more::Display,
    Debug,
    ProvidesStaticType,
    NoSerialize,
    Trace,
    Allocative,
    starlark::StarlarkPagable
)]
#[display("{}", RefCell::borrow(_0))]
pub struct StarlarkExecutionGroup(RefCell<InnerStarlarkExecutionGroup>);

#[starlark_value(skip_vtable)]
impl<'v> StarlarkValue<'v> for StarlarkExecutionGroup {
    type Type = StarlarkExecutionGroupView<'v>;
}

starlark::declare_starlark_value_as_type!(pub EXEC_GROUP_TYPE, StarlarkExecutionGroup);

const DEFAULT_EXECUTION_GROUP_NAME: &str = "DefaultExecGroup";

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ExecutionGroupError {
    #[error("Execution group has not yet been assigned to a global")]
    NotBound,
    #[error(
        "`DefaultExecGroup` names the built-in default execution group and cannot be assigned an `exec_group()`"
    )]
    ReservedName,
}

impl StarlarkExecutionGroup {
    /// A group that `export_as` names after the global it is assigned to.
    pub fn new_unbound(defining_file: CellPath) -> Self {
        Self(RefCell::new(InnerStarlarkExecutionGroup::Unbound(
            defining_file,
        )))
    }

    fn expect_bound(&self) -> buck2_error::Result<NamedExecutionGroup> {
        match &*self.0.borrow() {
            InnerStarlarkExecutionGroup::Unbound(_) => Err(ExecutionGroupError::NotBound.into()),
            InnerStarlarkExecutionGroup::Bound(group) => Ok(group.dupe()),
        }
    }
}

impl<'v> AllocValue<'v> for StarlarkExecutionGroup {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex(self)
    }
}

#[derive(
    derive_more::Display,
    Debug,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("<execution_group {_0}>")]
pub struct FrozenStarlarkExecutionGroup(#[starlark_pagable(pagable)] ExecutionGroup);
starlark_simple_value!(FrozenStarlarkExecutionGroup);

impl FrozenStarlarkExecutionGroup {
    pub const DEFAULT: Self = FrozenStarlarkExecutionGroup(ExecutionGroup::Default);
}

#[starlark_value]
impl<'v> StarlarkValue<'v> for FrozenStarlarkExecutionGroup {
    type Type = StarlarkExecutionGroupView<'v>;
}

impl<'v> Freeze<'v> for StarlarkExecutionGroup {
    type Frozen<'fv> = FrozenStarlarkExecutionGroup;
    fn freeze<'fv>(self, _: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        self.expect_bound()
            .map(|group| FrozenStarlarkExecutionGroup(ExecutionGroup::Named(group)))
            .map_err(|e| FreezeError::new(e.to_string()))
    }
}

/// An execution group in either of its representations: a group is bound to a name by
/// `export_as`, and the binding is required by the time the defining module freezes.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum StarlarkExecutionGroupView<'v> {
    Frozen(ValueTyped<'v, FrozenStarlarkExecutionGroup>),
    Unfrozen(ValueTyped<'v, StarlarkExecutionGroup>),
}

#[starlark_value(type = "ExecGroup")]
impl<'v> StarlarkType<'v> for StarlarkExecutionGroupView<'v> {
    fn export_as(
        &self,
        variable_name: &str,
        _eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<()> {
        let StarlarkExecutionGroupView::Unfrozen(this) = self else {
            return Ok(());
        };
        let mut inner = this.0.borrow_mut();
        let InnerStarlarkExecutionGroup::Unbound(cell_path) = &*inner else {
            // Was already exported
            return Ok(());
        };
        if variable_name == DEFAULT_EXECUTION_GROUP_NAME {
            return Err(buck2_error::Error::from(ExecutionGroupError::ReservedName).into());
        }
        let group = NamedExecutionGroup::new(variable_name.to_owned(), *cell_path);
        *inner = InnerStarlarkExecutionGroup::Bound(group);
        Ok(())
    }
}

impl StarlarkExecutionGroupView<'_> {
    fn expect_bound(&self) -> buck2_error::Result<ExecutionGroup> {
        match self {
            StarlarkExecutionGroupView::Unfrozen(v) => Ok(ExecutionGroup::Named(v.expect_bound()?)),
            StarlarkExecutionGroupView::Frozen(v) => Ok(v.0.dupe()),
        }
    }
}

#[derive(ProvidesStaticType)]
pub struct ExecutionGroupArg {
    pub group: ExecutionGroup,
}

impl StarlarkTypeRepr for ExecutionGroupArg {
    type Canonical = <StarlarkExecutionGroupView<'static> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        StarlarkExecutionGroupView::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for ExecutionGroupArg {
    type Error = starlark::Error;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        let Some(group) = StarlarkExecutionGroupView::unpack_value_opt(value) else {
            return Ok(None);
        };
        Ok(Some(ExecutionGroupArg {
            group: group.expect_bound()?,
        }))
    }
}
