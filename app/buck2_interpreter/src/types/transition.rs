/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::sync::Arc;

use buck2_core::configuration::transition::id::TransitionId;
use starlark::any::ProvidesStaticType;
use starlark::typing::Ty;
use starlark::values::StarlarkTypeExpr;
use starlark::values::StarlarkTypeExprStable;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::type_expr::UnionPayload;

use crate::late_binding_ty::TransitionReprLate;

/// Implemented by starlark transition objects.
pub trait TransitionValue {
    fn transition_id(&self) -> buck2_error::Result<Arc<TransitionId>>;
}

unsafe impl<'v> ProvidesStaticType<'v> for &'v dyn TransitionValue {
    type StaticType = &'static dyn TransitionValue;
}

/// A value that is a transition, in whichever representation the transition crate gives it.
pub struct ValueAsTransition<'v>(pub &'v dyn TransitionValue);

impl<'v> StarlarkTypeExpr<'v> for ValueAsTransition<'v> {
    type Get = Self;

    fn starlark_type_repr() -> Ty {
        TransitionReprLate::ty()
    }

    fn get(value: Value<'v>) -> Option<Self> {
        value
            .request_value::<&dyn TransitionValue>()
            .map(ValueAsTransition)
    }
}

impl<'v> StarlarkTypeExprStable<'v> for ValueAsTransition<'v> {}

impl<'v> UnionPayload<'v> for ValueAsTransition<'v> {
    type Expr = Self;

    fn from_value(value: Value<'v>) -> Option<Self> {
        <Self as StarlarkTypeExpr<'v>>::get(value)
    }
}

impl<'v> UnpackValue<'v> for ValueAsTransition<'v> {
    type Type = Self;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, Self>, get: Self) -> Result<Self, Infallible> {
        Ok(get)
    }
}

impl ValueAsTransition<'_> {
    pub fn transition_id(&self) -> buck2_error::Result<Arc<TransitionId>> {
        self.0.transition_id()
    }
}
