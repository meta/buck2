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

//! Unpackers whose verdict cannot change.

use either::Either;
use num_bigint::BigInt;

use crate::values::StarlarkValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::dict::DictRef;
use crate::values::float::StarlarkFloat;
use crate::values::float::UnpackFloat;
use crate::values::layout::typed::FrozenValueTyped;
use crate::values::list::ListRef;
use crate::values::none::NoneOr;
use crate::values::none::NoneType;
use crate::values::set::SetRef;
use crate::values::structs::StructRef;
use crate::values::tuple::TupleRef;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::int::inline_int::InlineInt;
use crate::values::types::int::int_or_big::StarlarkIntRef;
use crate::values::typing::StarlarkCallable;
use crate::values::typing::StarlarkCallableChecked;
use crate::values::typing::StarlarkCallableParamSpec;
use crate::values::typing::StarlarkIter;
use crate::values::typing::TypeType;

/// An [`UnpackValue`] whose verdict for a value cannot change over the value's lifetime.
///
/// The check depends only on what is fixed once a value exists: its type, the contents of an
/// immutable value, or a property of its type such as implementing a trait. It does not look
/// into a mutable container, whose contents Starlark may change later: `list[int]` is a finer
/// type than the representation `list`, and a check for it is not stable, while a check for
/// `list` is. [`ValueOf::unpack`](crate::values::ValueOf::unpack) is infallible exactly for
/// these.
///
/// The name follows `UnpackValue`'s; both change together when that trait is renamed for what it
/// is, the Rust vocabulary of Starlark types.
pub trait UnpackValueStable<'v>: UnpackValue<'v> {}

impl<'v> UnpackValueStable<'v> for Value<'v> {}
impl<'v, T: StarlarkValue<'v>> UnpackValueStable<'v> for ValueTyped<'v, T> {}
impl<'v, T: StarlarkValue<'v>> UnpackValueStable<'v> for FrozenValueTyped<'v, T> {}
impl<'v, T: StarlarkValue<'v>> UnpackValueStable<'v> for &'v T where &'v T: UnpackValue<'v> {}
impl<'v, T: UnpackValueStable<'v>> UnpackValueStable<'v> for ValueOf<'v, T> {}

impl<'v> UnpackValueStable<'v> for &'v ListRef<'v> {}
impl<'v> UnpackValueStable<'v> for &'v TupleRef<'v> {}
impl<'v> UnpackValueStable<'v> for DictRef<'v> {}
impl<'v> UnpackValueStable<'v> for SetRef<'v> {}
impl<'v> UnpackValueStable<'v> for StructRef<'v> {}

impl<'v> UnpackValueStable<'v> for &'v str {}
impl<'v> UnpackValueStable<'v> for String {}
impl<'v> UnpackValueStable<'v> for bool {}
impl<'v> UnpackValueStable<'v> for i32 {}
impl<'v> UnpackValueStable<'v> for u32 {}
impl<'v> UnpackValueStable<'v> for i64 {}
impl<'v> UnpackValueStable<'v> for u64 {}
impl<'v> UnpackValueStable<'v> for usize {}
impl<'v> UnpackValueStable<'v> for isize {}
impl<'v> UnpackValueStable<'v> for BigInt {}
impl<'v> UnpackValueStable<'v> for UnpackFloat {}
impl<'v> UnpackValueStable<'v> for StarlarkFloat {}
impl<'v> UnpackValueStable<'v> for StarlarkIntRef<'v> {}
impl<'v> UnpackValueStable<'v> for InlineInt {}
impl<'v> UnpackValueStable<'v> for NoneType {}
impl<'v> UnpackValueStable<'v> for TypeType {}
impl<'v, T: StarlarkTypeRepr> UnpackValueStable<'v> for StarlarkIter<T> {}

impl<'v, T: UnpackValueStable<'v>> UnpackValueStable<'v> for NoneOr<T> {}
impl<'v, A: UnpackValueStable<'v>, B: UnpackValueStable<'v>> UnpackValueStable<'v>
    for Either<A, B>
{
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeRepr> UnpackValueStable<'v>
    for StarlarkCallable<'v, P, R>
{
}
impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeRepr> UnpackValueStable<'v>
    for StarlarkCallableChecked<'v, P, R>
{
}
