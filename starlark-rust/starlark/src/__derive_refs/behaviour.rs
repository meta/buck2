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

//! What `#[starlark_value]` calls for a [`StarlarkType`] method written with a typed signature.
//!
//! The macro turns the typed method `foo` into the trait's method `foo`, whose body is one call
//! to `typed_foo` here, handing it the typed body as a closure. Each function converts the
//! operand and says what a value that is not of the operand's type means for its method, which
//! is what the trait's default for the method does.

use std::cmp::Ordering;

use crate::collections::StarlarkHasher;
use crate::values::StarlarkType;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::unpack_then;

/// What a typed method may return where the trait's method returns `starlark::Result<T>`: a
/// `T`, for a body that cannot fail, or the `starlark::Result<T>`.
#[diagnostic::on_unimplemented(
    message = "a typed `StarlarkType` method returns `{T}` or `starlark::Result<{T}>` here, not `{Self}`"
)]
pub trait TypedResult<T> {
    fn into_result(self) -> crate::Result<T>;
}

impl<T> TypedResult<T> for T {
    #[inline(always)]
    fn into_result(self) -> crate::Result<T> {
        Ok(self)
    }
}

impl<T> TypedResult<T> for crate::Result<T> {
    #[inline(always)]
    fn into_result(self) -> crate::Result<T> {
        self
    }
}

/// [`StarlarkType::equals`] from a typed body. A value that is not of the operand's type is not
/// equal.
#[inline(always)]
pub fn typed_equals<'v, O, R>(other: Value<'v>, typed: impl FnOnce(O) -> R) -> crate::Result<bool>
where
    O: UnpackValue<'v>,
    R: TypedResult<bool>,
{
    unpack_then(other, |other| typed(other).into_result(), || Ok(false))
}

/// [`StarlarkType::compare`] from a typed body. A value that is not of the operand's type does
/// not compare: [`compare_unsupported`].
#[inline(always)]
pub fn typed_compare<'v, T, O, R>(
    this: &T,
    other: Value<'v>,
    typed: impl FnOnce(O) -> R,
) -> crate::Result<Ordering>
where
    T: StarlarkType<'v>,
    O: UnpackValue<'v>,
    R: TypedResult<Ordering>,
{
    unpack_then(
        other,
        |other| typed(other).into_result(),
        || compare_unsupported(this, other),
    )
}

/// The error for comparing `this` with a value it does not compare with: the default of
/// [`StarlarkType::compare`], and the answer of a typed `compare` for a value that is not of
/// its operand's type.
#[cold]
pub fn compare_unsupported<'v, T: StarlarkType<'v>>(
    this: &T,
    other: Value<'v>,
) -> crate::Result<Ordering> {
    ValueError::unsupported_with(this, "compare", other)
}

/// [`StarlarkType::write_hash`] from a body that cannot fail.
#[inline(always)]
pub fn typed_write_hash(
    hasher: &mut StarlarkHasher,
    typed: impl FnOnce(&mut StarlarkHasher),
) -> crate::Result<()> {
    typed(hasher);
    Ok(())
}
