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

//! Bindings to/from Rust tuple types.

use either::Either;

use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::types::tuple::value::Tuple;

impl<'v, T1: AllocValue<'v>> AllocValue<'v> for (T1,) {
    type Type = (T1::Type,);

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        // The items are the parts' `Type`s by their declarations.
        ValueOf::new_unchecked(heap.alloc_tuple(&[self.0.alloc_value(heap).to_value()]))
    }
}

impl<'v, T1: AllocValue<'v>, T2: AllocValue<'v>> AllocValue<'v> for (T1, T2) {
    type Type = (T1::Type, T2::Type);

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        // The items are the parts' `Type`s by their declarations.
        ValueOf::new_unchecked(heap.alloc_tuple(&[
            self.0.alloc_value(heap).to_value(),
            self.1.alloc_value(heap).to_value(),
        ]))
    }
}

impl<'v, T1: AllocValue<'v>, T2: AllocValue<'v>, T3: AllocValue<'v>> AllocValue<'v>
    for (T1, T2, T3)
{
    type Type = (T1::Type, T2::Type, T3::Type);

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        // The items are the parts' `Type`s by their declarations.
        ValueOf::new_unchecked(heap.alloc_tuple(&[
            self.0.alloc_value(heap).to_value(),
            self.1.alloc_value(heap).to_value(),
            self.2.alloc_value(heap).to_value(),
        ]))
    }
}

impl<'fv, T1: AllocFrozenValue<'fv>> AllocFrozenValue<'fv> for (T1,) {
    type Type = (T1::Type,);

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        // The items are the parts' `Type`s by their declarations.
        ValueOf::new_unchecked(heap.alloc_tuple(&[self.0.alloc_frozen_value(heap).to_value()]))
    }
}

impl<'fv, T1: AllocFrozenValue<'fv>, T2: AllocFrozenValue<'fv>> AllocFrozenValue<'fv> for (T1, T2) {
    type Type = (T1::Type, T2::Type);

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        // The items are the parts' `Type`s by their declarations.
        ValueOf::new_unchecked(heap.alloc_tuple(&[
            self.0.alloc_frozen_value(heap).to_value(),
            self.1.alloc_frozen_value(heap).to_value(),
        ]))
    }
}

impl<'fv, T1: AllocFrozenValue<'fv>, T2: AllocFrozenValue<'fv>, T3: AllocFrozenValue<'fv>>
    AllocFrozenValue<'fv> for (T1, T2, T3)
{
    type Type = (T1::Type, T2::Type, T3::Type);

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        // The items are the parts' `Type`s by their declarations.
        ValueOf::new_unchecked(heap.alloc_tuple(&[
            self.0.alloc_frozen_value(heap).to_value(),
            self.1.alloc_frozen_value(heap).to_value(),
            self.2.alloc_frozen_value(heap).to_value(),
        ]))
    }
}

impl<'v, T1: StarlarkTypeExpr<'v>, T2: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for (T1, T2) {
    type Get = (T1::Get, T2::Get);

    fn starlark_type_repr() -> Ty {
        Ty::tuple2(T1::starlark_type_repr(), T2::starlark_type_repr())
    }

    fn get(value: Value<'v>) -> Option<Self::Get> {
        match Tuple::from_value(value)?.content() {
            [a, b] => Some((T1::get(*a)?, T2::get(*b)?)),
            _ => None,
        }
    }
}

impl<'v, T1: StarlarkTypeExprStable<'v>, T2: StarlarkTypeExprStable<'v>> StarlarkTypeExprStable<'v>
    for (T1, T2)
{
}

impl<'v, T1: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for (T1,) {
    type Get = (T1::Get,);

    fn starlark_type_repr() -> Ty {
        Ty::tuple(vec![T1::starlark_type_repr()])
    }

    fn get(value: Value<'v>) -> Option<Self::Get> {
        match Tuple::from_value(value)?.content() {
            [a] => Some((T1::get(*a)?,)),
            _ => None,
        }
    }
}

impl<'v, T1: StarlarkTypeExprStable<'v>> StarlarkTypeExprStable<'v> for (T1,) {}

impl<'v, T1: StarlarkTypeExpr<'v>, T2: StarlarkTypeExpr<'v>, T3: StarlarkTypeExpr<'v>>
    StarlarkTypeExpr<'v> for (T1, T2, T3)
{
    type Get = (T1::Get, T2::Get, T3::Get);

    fn starlark_type_repr() -> Ty {
        Ty::tuple(vec![
            T1::starlark_type_repr(),
            T2::starlark_type_repr(),
            T3::starlark_type_repr(),
        ])
    }

    fn get(value: Value<'v>) -> Option<Self::Get> {
        match Tuple::from_value(value)?.content() {
            [a, b, c] => Some((T1::get(*a)?, T2::get(*b)?, T3::get(*c)?)),
            _ => None,
        }
    }
}

impl<
    'v,
    T1: StarlarkTypeExprStable<'v>,
    T2: StarlarkTypeExprStable<'v>,
    T3: StarlarkTypeExprStable<'v>,
> StarlarkTypeExprStable<'v> for (T1, T2, T3)
{
}

/// The items of a tuple value known to have `N` of them.
fn items<'v, const N: usize>(value: Value<'v>) -> [Value<'v>; N] {
    Tuple::from_value(value)
        .and_then(|t| t.content().try_into().ok())
        .expect("a value's type and a tuple's length do not change")
}

impl<'v, T1: UnpackValue<'v>, T2: UnpackValue<'v>> UnpackValue<'v> for (T1, T2) {
    type Type = (T1::Type, T2::Type);
    type Error = Either<T1::Error, T2::Error>;

    fn unpack(
        value: ValueOf<'v, Self::Type>,
        (a, b): <Self::Type as StarlarkTypeExpr<'v>>::Get,
    ) -> Result<Self, Self::Error> {
        let [va, vb] = items(value.to_value());
        Ok((
            T1::unpack(ValueOf::new_unchecked(va), a).map_err(Either::Left)?,
            T2::unpack(ValueOf::new_unchecked(vb), b).map_err(Either::Right)?,
        ))
    }
}

impl<'v, T1: UnpackValue<'v>, T2: UnpackValue<'v>, T3: UnpackValue<'v>> UnpackValue<'v>
    for (T1, T2, T3)
{
    type Type = (T1::Type, T2::Type, T3::Type);
    type Error = Either<T1::Error, Either<T2::Error, T3::Error>>;

    fn unpack(
        value: ValueOf<'v, Self::Type>,
        (a, b, c): <Self::Type as StarlarkTypeExpr<'v>>::Get,
    ) -> Result<Self, Self::Error> {
        let [va, vb, vc] = items(value.to_value());
        Ok((
            T1::unpack(ValueOf::new_unchecked(va), a).map_err(Either::Left)?,
            T2::unpack(ValueOf::new_unchecked(vb), b)
                .map_err(Either::Left)
                .map_err(Either::Right)?,
            T3::unpack(ValueOf::new_unchecked(vc), c)
                .map_err(Either::Right)
                .map_err(Either::Right)?,
        ))
    }
}
