/*
 * Copyright 2018 The Starlark in Rust Authors.
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

use std::convert::Infallible;

use dupe::Dupe;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::AllocValue;
use crate::values::Heap;
use crate::values::StringValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::starlark_type_id::StarlarkTypeId;
use crate::values::structs::AllocStruct;
use crate::values::structs::value::Struct;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::type_repr::StarlarkTypeRepr;

/// Reference to a struct allocated on the heap.
///
/// Struct implementation (for example, memory layout) may change,
/// this type provides implementation agnostics API to it.
#[derive(Debug, Clone, Copy, Dupe, ProvidesStaticType)]
pub struct StructRef<'v>(&'v Struct<'v>);

impl<'v> StructRef<'v> {
    /// Downcast a value to a struct reference.
    pub fn from_value(value: Value<'v>) -> Option<StructRef<'v>> {
        Struct::from_value(value).map(StructRef)
    }

    #[inline]
    pub(crate) fn is_instance(value: Value<'v>) -> bool {
        value.starlark_type_id() == StarlarkTypeId::of::<Struct>()
    }

    /// Iterate over struct fields.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (StringValue<'v>, Value<'v>)> + '_ {
        self.0.iter()
    }
}

impl<'v> ValueOf<'v, StructRef<'v>> {
    /// A new struct of these fields.
    pub fn alloc_struct<S>(heap: Heap<'v>, fields: AllocStruct<S>) -> Self
    where
        AllocStruct<S>: AllocValue<'v>,
    {
        Self::new_unchecked(heap.alloc(fields))
    }
}

impl<'v> StarlarkTypeRepr for StructRef<'v> {
    type Canonical = Self;

    fn starlark_type_repr() -> Ty {
        Struct::starlark_type_repr()
    }
}

/// The handle to a struct, from the struct it was allocated as.
impl<'v> From<ValueOf<'v, Struct<'v>>> for ValueOf<'v, StructRef<'v>> {
    #[inline]
    fn from(value: ValueOf<'v, Struct<'v>>) -> Self {
        Self::new_unchecked(value.to_value())
    }
}

impl<'v> StarlarkTypeExpr<'v> for StructRef<'v> {
    type Get = Self;

    #[inline]
    fn get(value: Value<'v>) -> Option<Self> {
        StructRef::from_value(value)
    }
}

impl<'v> StarlarkTypeExprStable<'v> for StructRef<'v> {}

impl<'v> UnpackValue<'v> for StructRef<'v> {
    type Type = Self;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, Self>, get: Self) -> Result<Self, Infallible> {
        Ok(get)
    }
}
