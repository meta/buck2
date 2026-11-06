/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Special value which holds a reference to actual value.
//! This is used to implement variable capture by nested functions.
//!
//! `Value` holding `ValueCaptured` is equivalent to `Box<Option<Value>>`.

use std::cell::Cell;

use allocative::Allocative;
use derive_more::Display;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::StarlarkRepresentations;
use starlark_derive::Trace;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueTyped;

#[derive(Debug, Trace, ProvidesStaticType, Display, NoSerialize, Allocative)]
#[display("{:?}", self)] // This type should never be user visible
#[repr(transparent)]
#[allocative(skip)]
pub(crate) struct ValueCaptured<'v>(Cell<Option<Value<'v>>>);

#[derive(
    Debug,
    ProvidesStaticType,
    Display,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{:?}", self)] // Type is not user visible
#[repr(transparent)]
pub(crate) struct FrozenValueCaptured<'v>(Option<Value<'v>>);

/// A captured variable in either of its representations.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub(crate) enum ValueCapturedView<'v> {
    Unfrozen(ValueTyped<'v, ValueCaptured<'v>>),
    Frozen(ValueTyped<'v, FrozenValueCaptured<'v>>),
}

#[starlark_value(type = "value_captured")]
impl<'v> StarlarkType<'v> for ValueCapturedView<'v> {}

#[starlark_value]
impl<'v> StarlarkValue<'v> for ValueCaptured<'v> {
    type Type = ValueCapturedView<'v>;
}

#[starlark_value(frozen_vtable)]
impl<'v> StarlarkValue<'v> for FrozenValueCaptured<'v> {
    type Canonical = ValueCaptured<'v>;
    type Type = ValueCapturedView<'v>;
}

impl<'v> ValueCaptured<'v> {
    pub(crate) fn new(payload: Option<Value<'v>>) -> ValueCaptured<'v> {
        if let Some(payload) = payload {
            debug_assert!(payload.downcast_ref::<ValueCaptured>().is_none());
            debug_assert!(payload.downcast_ref::<FrozenValueCaptured>().is_none());
        }
        ValueCaptured(Cell::new(payload))
    }

    pub(crate) fn set(&self, value: Value<'v>) {
        debug_assert!(value.downcast_ref::<ValueCaptured>().is_none());
        debug_assert!(value.downcast_ref::<FrozenValueCaptured>().is_none());
        self.0.set(Some(value));
    }
}

impl<'v> Freeze<'v> for ValueCaptured<'v> {
    type Frozen<'fv> = FrozenValueCaptured<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<FrozenValueCaptured<'fv>> {
        Ok(FrozenValueCaptured(
            self.0.get().map(|v| freezer.freeze(v)).transpose()?,
        ))
    }
}

pub(crate) fn value_captured_get<'v>(value_captured: Value<'v>) -> Option<Value<'v>> {
    if let Some(value_captured) = value_captured.downcast_ref::<FrozenValueCaptured<'v>>() {
        value_captured.0
    } else {
        value_captured
            .downcast_ref::<ValueCaptured<'v>>()
            .expect("not a ValueCaptured")
            .0
            .get()
    }
}
