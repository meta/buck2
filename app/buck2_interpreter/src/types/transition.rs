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
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::type_repr::StarlarkTypeRepr;

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

impl StarlarkTypeRepr for ValueAsTransition<'_> {
    type Canonical = TransitionReprLate;

    fn starlark_type_repr() -> Ty {
        TransitionReprLate::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for ValueAsTransition<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(value
            .request_value::<&dyn TransitionValue>()
            .map(ValueAsTransition))
    }
}

impl ValueAsTransition<'_> {
    pub fn transition_id(&self) -> buck2_error::Result<Arc<TransitionId>> {
        self.0.transition_id()
    }
}
