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

use crate::__derive_refs::param_spec::param_default_at;
use crate::eval::Arguments;
use crate::eval::ParametersSpec;
use crate::values::Heap;
use crate::values::ParamDefault;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::dict::AllocDict;
use crate::values::type_expr::Accepts;

/// Collect `N` arguments.
///
/// This function is called by generated code.
#[inline]
pub fn parse_signature<'v, const N: usize>(
    parser: &ParametersSpec<Value<'v>>,
    args: &Arguments<'v, '_>,
    heap: Heap<'v>,
) -> crate::Result<[Option<Value<'v>>; N]> {
    parser.collect_into(args, heap)
}

/// Parse positional-only arguments, required and optional.
#[inline(always)]
pub fn parse_positional<'v, const R: usize, const O: usize>(
    args: &Arguments<'v, '_>,
    heap: Heap<'v>,
) -> crate::Result<([Value<'v>; R], [Option<Value<'v>>; O])> {
    args.no_named_args()?;
    args.optional(heap)
}

#[inline(always)]
pub fn parse_positional_kwargs_alloc<'v, 'a, const R: usize, const O: usize>(
    args: &'a Arguments<'v, 'a>,
    heap: Heap<'v>,
) -> crate::Result<([Value<'v>; R], [Option<Value<'v>>; O], Value<'v>)> {
    let (required, optional) = args.optional(heap)?;
    let kwargs = args.names_map()?;
    let kwargs = heap.alloc(AllocDict(kwargs)).to_value();
    Ok((required, optional, kwargs))
}

/// Converts the receiver of a method to the type the method declares for it.
#[inline(always)]
pub fn check_this<'v, T: UnpackValue<'v>>(this: Value<'v>) -> crate::Result<T> {
    T::unpack_named_param(this, "this")
}

/// Not generic and out of line: `check_required` is inlined into every native function, once
/// per parameter.
#[cold]
#[inline(never)]
fn missing_required(name: &str) -> crate::Error {
    ValueError::MissingRequired(name.to_owned()).into()
}

/// Utility for checking a required parameter matches what you expect.
#[inline(always)]
pub fn check_required<'v, T: UnpackValue<'v>>(
    name: &str,
    x: Option<Value<'v>>,
) -> crate::Result<T> {
    match x {
        Some(x) => T::unpack_named_param(x, name),
        None => Err(missing_required(name)),
    }
}

/// Utility for checking an optional parameter matches what you expect.
#[inline(always)]
pub fn check_optional<'v, T: UnpackValue<'v>>(
    name: &str,
    x: Option<Value<'v>>,
) -> crate::Result<Option<T>> {
    match x {
        None => Ok(None),
        Some(x) => Ok(Some(T::unpack_named_param(x, name)?)),
    }
}

/// A default that is of the type of a parameter of type `T`: the type the parameter converts
/// from [takes](Accepts) the default's. `Path` is that relation's and is inferred.
///
/// This is what makes a default of the wrong type a compile error. It is a trait of its own,
/// in the author's Rust types, so that the error says what the author wrote and is reported
/// once, at the default.
#[diagnostic::on_unimplemented(
    message = "the default `{Self}` is not of the type of its parameter, `{T}`",
    label = "this default is not of the parameter's type",
    note = "a default is written as the Starlark value: `NoneType` for a `NoneOr<T>` parameter, `EmptyList` for a list, and for a union a value of one of its alternatives"
)]
pub trait DefaultFor<'v, T, Path> {}

#[diagnostic::do_not_recommend]
impl<'v, T: UnpackValue<'v>, D: ParamDefault, Path> DefaultFor<'v, T, Path> for D where
    T::Type: Accepts<'v, D::Type<'v>, Path>
{
}

/// Converts the argument of a parameter that has a default: an argument that was left out is
/// the default.
///
/// The two arms convert separately, where converting `x.unwrap_or_else(default)` would be
/// less code: a default is a constant, and its conversion folds only if it is not merged with
/// that of an argument.
#[inline(always)]
pub fn check_defaulted<'v, T: UnpackValue<'v>, D: ParamDefault + DefaultFor<'v, T, Path>, Path>(
    name: &str,
    x: Option<Value<'v>>,
    default: impl FnOnce() -> D,
) -> crate::Result<T> {
    match x {
        None => T::unpack_named_param(param_default_at(default()), name),
        Some(x) => T::unpack_named_param(x, name),
    }
}

/// We already know the parameter is set, so we just unpack it.
#[inline(always)]
pub fn check_unpack<'v, T: UnpackValue<'v>>(name: &str, x: Value<'v>) -> crate::Result<T> {
    T::unpack_named_param(x, name)
}
