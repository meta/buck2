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

use allocative::Allocative;
use dupe::Dupe;
use either::Either;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::none::NoneType;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;

/// Equivalent of a Rust [`Option`], where `None`
/// is encoded as [`NoneType`](crate::values::none::NoneType).
/// Useful for its [`UnpackValue`] instance.
#[derive(
    Debug,
    Eq,
    PartialEq,
    Copy,
    Clone,
    Dupe,
    Allocative,
    ProvidesStaticType
)]
pub enum NoneOr<T> {
    /// Starlark `None`.
    None,
    /// Not `None`.
    Other(T),
}

impl<T> NoneOr<T> {
    /// Convert the [`NoneOr`] to a real Rust [`Option`].
    #[inline]
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::None => None,
            Self::Other(x) => Some(x),
        }
    }

    /// Convert a Rust [`Option`] to a [`NoneOr`].
    #[inline]
    pub fn from_option(option: Option<T>) -> Self {
        match option {
            None => NoneOr::None,
            Some(x) => NoneOr::Other(x),
        }
    }

    /// Is the value a [`NoneOr::None`].
    pub fn is_none(&self) -> bool {
        matches!(self, NoneOr::None)
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for NoneOr<T> {
    type Get = Option<T::Get>;

    fn starlark_type_repr() -> Ty {
        Either::<NoneType, T>::starlark_type_repr()
    }

    fn get(value: Value<'v>) -> Option<Self::Get> {
        if value.is_none() {
            Some(None)
        } else {
            T::get(value).map(Some)
        }
    }
}

impl<'v, T: StarlarkTypeExprStable<'v>> StarlarkTypeExprStable<'v> for NoneOr<T> {}

impl<'v, T: UnpackValue<'v>> UnpackValue<'v> for NoneOr<T> {
    type Type = NoneOr<T::Type>;
    type Error = T::Error;

    fn unpack(
        value: ValueOf<'v, Self::Type>,
        get: Option<<T::Type as StarlarkTypeExpr<'v>>::Get>,
    ) -> Result<Self, Self::Error> {
        match get {
            None => Ok(NoneOr::None),
            Some(get) => {
                T::unpack(ValueOf::new_unchecked(value.to_value()), get).map(NoneOr::Other)
            }
        }
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, NoneOr<T>> {
    /// `None`.
    #[inline]
    pub fn none() -> Self {
        Self::new_unchecked(Value::new_none())
    }

    /// A value of `T`, as a value of `None | T`.
    #[inline]
    pub fn some(value: ValueOf<'v, T>) -> Self {
        Self::new_unchecked(value.to_value())
    }
}

impl<'v, T: AllocValue<'v>> AllocValue<'v> for NoneOr<T> {
    type Type = NoneOr<T::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        match self {
            NoneOr::None => ValueOf::none(),
            NoneOr::Other(x) => ValueOf::some(x.alloc_value(heap)),
        }
    }
}

impl<'fv, T: AllocFrozenValue<'fv>> AllocFrozenValue<'fv> for NoneOr<T> {
    type Type = NoneOr<T::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        match self {
            NoneOr::None => ValueOf::none(),
            NoneOr::Other(x) => ValueOf::some(x.alloc_frozen_value(heap)),
        }
    }
}
