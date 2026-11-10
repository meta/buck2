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

//! Parameter conversion utilities for `starlark_module` macros.

use std::convert::Infallible;
use std::fmt::Debug;

use anyhow::Context;
use either::Either;
use starlark_syntax::StarlarkResultExt;

use crate::typing::Ty;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_repr::StarlarkTypeRepr;

/// The error for a value that is not of the type `ty` names.
#[cold]
pub(crate) fn expected_type_error(value: Value<'_>, ty: fn() -> Ty) -> crate::Error {
    #[derive(thiserror::Error, Debug)]
    #[error("Expected `{0}`, but got `{1}`")]
    struct IncorrectType(Ty, String);

    crate::Error::new_runtime_type(
        crate::RuntimeTypeErrorKind::ValueUnpack,
        IncorrectType(ty(), value.to_string_for_type_error()),
    )
}

/// `matched` is what `T::matches` said about `value`; it must be what the declared type says.
#[cfg(debug_assertions)]
pub(crate) fn check_matches_agreement<'v, T: StarlarkTypeExpr<'v>>(
    value: Value<'v>,
    matched: bool,
) {
    use crate::values::typing::type_compiled::matcher::TypeMatcher;

    let by_type = type_matcher::<T>().matches(value);
    if by_type != matched {
        panic!(
            "`get` disagrees with the declared type: Rust type `{rust_type}` declares Starlark \
             type `{ty}`, which {by_type} `{value}`, but `get` {matched} it",
            rust_type = std::any::type_name::<T>(),
            ty = T::starlark_type_repr(),
            by_type = if by_type { "accepts" } else { "rejects" },
            value = value.to_string_for_type_error(),
            matched = if matched { "accepted" } else { "rejected" },
        );
    }
}

/// The error for a `ValueOf<T>` whose value is no longer a `T`: a mutable container that was
/// changed after it was checked.
#[cold]
pub(crate) fn changed_since_checked_error<'v, T: StarlarkTypeExpr<'v>>(
    value: Value<'v>,
) -> crate::Error {
    #[derive(thiserror::Error, Debug)]
    #[error("Value `{1}` was changed after it was checked: it is no longer a `{0}`")]
    struct ChangedSinceChecked(Ty, String);

    crate::Error::new_runtime_type(
        crate::RuntimeTypeErrorKind::ValueUnpack,
        ChangedSinceChecked(T::starlark_type_repr(), value.to_string_for_type_error()),
    )
}

/// The runtime matcher for `T::starlark_type_repr()`, built once per Rust type: what the debug
/// checks compare an implementation against.
#[cfg(any(debug_assertions, test))]
pub(crate) fn type_matcher<T: StarlarkTypeRepr + ?Sized>()
-> std::sync::Arc<crate::values::typing::type_compiled::matcher::TypeMatcherBox> {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    use dupe::Dupe;

    use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
    use crate::values::typing::type_compiled::matcher::TypeMatcherBox;
    use crate::values::typing::type_compiled::matcher::TypeMatcherBoxAlloc;

    // Keyed by the type name: `TypeId::of` wants `'static`, the handle types carry `'v`, and
    // `StarlarkTypeRepr` does not require the `ProvidesStaticType` that would erase it. The name
    // erases lifetimes too, and they cannot change the `Ty`.
    static MATCHERS: OnceLock<Mutex<HashMap<&'static str, Arc<TypeMatcherBox>>>> = OnceLock::new();

    let rust_type = std::any::type_name::<T>();
    let matchers = MATCHERS.get_or_init(Default::default);
    let matcher = matchers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(rust_type)
        .map(Dupe::dupe);
    match matcher {
        Some(matcher) => matcher,
        None => {
            // Built outside the lock: a type repr may do arbitrary work.
            let matcher = Arc::new(TypeMatcherBoxAlloc.ty(&T::starlark_type_repr()));
            matchers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(rust_type)
                .or_insert(matcher)
                .dupe()
        }
    }
}

/// Error that can be returned by [`UnpackValue`].
pub trait UnpackValueError: Debug + Send + Sync + 'static {
    /// Convert into a crate error.
    fn into_error(this: Self) -> crate::Error;
}

impl UnpackValueError for crate::Error {
    #[cold]
    fn into_error(this: Self) -> crate::Error {
        this
    }
}

impl UnpackValueError for anyhow::Error {
    #[cold]
    fn into_error(this: Self) -> crate::Error {
        crate::Error::new_value(this)
    }
}

impl UnpackValueError for Infallible {
    #[cold]
    fn into_error(this: Self) -> crate::Error {
        match this {}
    }
}

impl<A: UnpackValueError, B: UnpackValueError> UnpackValueError for Either<A, B> {
    #[cold]
    fn into_error(this: Self) -> crate::Error {
        match this {
            Either::Left(a) => UnpackValueError::into_error(a),
            Either::Right(b) => UnpackValueError::into_error(b),
        }
    }
}

/// Never error.
pub trait UnpackValueErrorInfallible: UnpackValueError {
    /// Convert into a never type.
    fn into_infallible(this: Self) -> !;
}

impl UnpackValueErrorInfallible for Infallible {
    fn into_infallible(this: Self) -> ! {
        match this {}
    }
}

impl<A: UnpackValueErrorInfallible, B: UnpackValueErrorInfallible> UnpackValueErrorInfallible
    for Either<A, B>
{
    fn into_infallible(this: Self) -> ! {
        match this {
            Either::Left(a) => UnpackValueErrorInfallible::into_infallible(a),
            Either::Right(b) => UnpackValueErrorInfallible::into_infallible(b),
        }
    }
}

/// A conversion from a Starlark type to a Rust type: how a parameter of a
/// [`#[starlark_module]`](macro@crate::starlark_module) function is bound.
///
/// [`Type`](Self::Type) is the Starlark type converted from, which is what the typechecker and
/// the documentation go by, and [`unpack`](Self::unpack) is handed a value already known to be
/// of it. A conversion can fail, but it cannot accept or reject differently from its type.
///
/// Most Rust types are conversions from themselves: a [`StarlarkTypeExpr`] whose
/// [`Get`](StarlarkTypeExpr::Get) is itself, such as a view, a `#[derive(UnpackValue)]` union
/// or [`Value`], and `&T` for a type `T` with one representation, which a blanket impl covers.
/// A Rust type that is not a type expression writes the impl:
///
/// ```
/// # use either::Either;
/// # use starlark::values::{UnpackValue, ValueOf};
/// # use starlark::values::int::IntView;
///
/// struct BoolOrInt(i32);
///
/// impl<'v> UnpackValue<'v> for BoolOrInt {
///     type Type = Either<bool, IntView<'v>>;
///     type Error = starlark::Error;
///
///     fn unpack(
///         _value: ValueOf<'v, Self::Type>,
///         get: Either<bool, IntView<'v>>,
///     ) -> starlark::Result<Self> {
///         match get {
///             Either::Left(b) => Ok(BoolOrInt(b as i32)),
///             Either::Right(int) => Ok(BoolOrInt(int.to_integer()?)),
///         }
///     }
/// }
/// ```
pub trait UnpackValue<'v>: Sized {
    /// The Starlark type converted from: what a parameter of this Rust type accepts.
    type Type: StarlarkTypeExpr<'v>;

    /// The error for a value that is of [`Type`](Self::Type) but does not convert, such as an
    /// `int` that does not fit an `i32`.
    ///
    /// Typically [`starlark::Error`](crate::Error), [`anyhow::Error`], or [`Infallible`].
    type Error: UnpackValueError;

    /// Converts `value`, which is of [`Type`](Self::Type), from `get`, what it was read as.
    ///
    /// Both are the same value. `get` is the reading that found the value to be of the type,
    /// handed on because a `ValueOf` of a type that is not
    /// [`StarlarkTypeExprStable`](crate::values::StarlarkTypeExprStable) cannot be read again
    /// without the reading possibly failing; with it, a conversion from such a type, and the
    /// `Either`, `NoneOr` and tuple combinators, stay total. `value` is for a conversion that
    /// keeps the value or reads it as something else.
    fn unpack(
        value: ValueOf<'v, Self::Type>,
        get: <Self::Type as StarlarkTypeExpr<'v>>::Get,
    ) -> Result<Self, Self::Error>;

    /// `value` converted, if it is of [`Type`](Self::Type): `None` if it is not, an error if it
    /// is but does not convert.
    fn unpack_value(value: Value<'v>) -> crate::Result<Option<Self>> {
        match ValueOf::<Self::Type>::new_with_get(value) {
            None => Ok(None),
            Some((value, get)) => Self::unpack(value, get)
                .map(Some)
                .map_err(Self::Error::into_error),
        }
    }

    /// [`unpack_value`](Self::unpack_value) for a conversion that cannot fail.
    fn unpack_value_opt(value: Value<'v>) -> Option<Self>
    where
        Self::Error: UnpackValueErrorInfallible,
    {
        let (value, get) = ValueOf::<Self::Type>::new_with_get(value)?;
        match Self::unpack(value, get) {
            Ok(x) => Some(x),
            Err(e) => Self::Error::into_infallible(e),
        }
    }

    /// [`unpack_value`](Self::unpack_value), with an error instead of `None`.
    #[inline]
    fn unpack_value_err(value: Value<'v>) -> crate::Result<Self> {
        Self::unpack_value(value)?
            .ok_or_else(|| expected_type_error(value, Self::Type::starlark_type_repr))
    }

    /// [`unpack_value`](Self::unpack_value), with an error about a parameter's type instead of
    /// `None`.
    #[inline]
    fn unpack_param(value: Value<'v>) -> crate::Result<Self> {
        #[cold]
        fn error<'v>(value: Value<'v>, ty: fn() -> Ty) -> crate::Error {
            #[derive(thiserror::Error, Debug)]
            #[error("Type of parameters mismatch, expected `{0}`, actual `{1}`")]
            struct IncorrectParameterTypeWithExpected(Ty, String);

            crate::Error::new_runtime_type(
                crate::RuntimeTypeErrorKind::Parameter,
                IncorrectParameterTypeWithExpected(ty(), value.to_string_for_type_error()),
            )
        }

        Self::unpack_value(value)?.ok_or_else(|| error(value, Self::Type::starlark_type_repr))
    }

    /// [`unpack_value`](Self::unpack_value), with an error about the named parameter's type
    /// instead of `None`.
    #[inline]
    fn unpack_named_param(value: Value<'v>, param_name: &str) -> crate::Result<Self> {
        #[cold]
        fn error<'v>(value: Value<'v>, param_name: &str, ty: fn() -> Ty) -> crate::Error {
            #[derive(thiserror::Error, Debug)]
            #[error("Type of parameter `{0}` doesn't match, expected `{1}`, actual `{2}`")]
            struct IncorrectParameterTypeNamedWithExpected(String, Ty, String);

            crate::Error::new_runtime_type(
                crate::RuntimeTypeErrorKind::Parameter,
                IncorrectParameterTypeNamedWithExpected(
                    param_name.to_owned(),
                    ty(),
                    value.to_string_for_type_error(),
                ),
            )
        }

        Self::unpack_value(value)
            .into_anyhow_result()
            .with_context(|| {
                format!(
                    "Error unpacking value for parameter `{}` of type `{}",
                    param_name,
                    Self::Type::starlark_type_repr()
                )
            })?
            .ok_or_else(|| error(value, param_name, Self::Type::starlark_type_repr))
    }
}

impl<'v> UnpackValue<'v> for Value<'v> {
    type Type = Value<'v>;
    type Error = Infallible;

    #[inline]
    fn unpack(value: ValueOf<'v, Value<'v>>, _get: Value<'v>) -> Result<Self, Infallible> {
        Ok(value.to_value())
    }
}

/// A reference to the one representation of a type, which is what a parameter of a type
/// declared with [`#[starlark_value]`](crate::values::starlark_value) is bound as. A type with
/// several representations has no reference to "the" representation; it is bound as its view
/// or as a [`ValueOf`].
impl<'v, T> UnpackValue<'v> for &'v T
where
    T: StarlarkValue<'v> + StarlarkType<'v, Get = ValueTyped<'v, T>>,
{
    type Type = T;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, T>, get: ValueTyped<'v, T>) -> Result<Self, Infallible> {
        Ok(get.as_ref())
    }
}

impl<'v, TLeft: UnpackValue<'v>, TRight: UnpackValue<'v>> UnpackValue<'v>
    for Either<TLeft, TRight>
{
    type Type = Either<TLeft::Type, TRight::Type>;
    type Error = Either<TLeft::Error, TRight::Error>;

    fn unpack(
        value: ValueOf<'v, Self::Type>,
        get: <Self::Type as StarlarkTypeExpr<'v>>::Get,
    ) -> Result<Self, Self::Error> {
        let value = value.to_value();
        match get {
            Either::Left(left) => TLeft::unpack(ValueOf::new_unchecked(value), left)
                .map(Either::Left)
                .map_err(Either::Left),
            Either::Right(right) => TRight::unpack(ValueOf::new_unchecked(value), right)
                .map(Either::Right)
                .map_err(Either::Right),
        }
    }
}

impl<'v, A: StarlarkTypeExpr<'v>, B: StarlarkTypeExpr<'v>> ValueOf<'v, Either<A, B>> {
    /// A value of `A`, as a value of the union.
    #[inline]
    pub fn left(value: ValueOf<'v, A>) -> Self {
        Self::new_unchecked(value.to_value())
    }

    /// A value of `B`, as a value of the union.
    #[inline]
    pub fn right(value: ValueOf<'v, B>) -> Self {
        Self::new_unchecked(value.to_value())
    }

    /// Which side of the union this value is, trying `A` first as [`get`](ValueOf::get) does,
    /// as handles rather than as [`get`](ValueOf::get)'s `Either` of the sides' handles.
    /// Fallible for the same reason as [`try_get`](ValueOf::try_get).
    pub fn split(self) -> crate::Result<Either<ValueOf<'v, A>, ValueOf<'v, B>>> {
        let value = self.to_value();
        if let Some(a) = ValueOf::<A>::new(value) {
            Ok(Either::Left(a))
        } else if let Some(b) = ValueOf::<B>::new(value) {
            Ok(Either::Right(b))
        } else {
            Err(expected_type_error(value, Self::starlark_type_repr))
        }
    }
}

#[cfg(test)]
mod tests {
    use starlark_derive::starlark_module;

    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::values::FrozenValueTyped;
    use crate::values::none::NoneType;
    use crate::values::record::record_type::FrozenRecordType;

    #[starlark_module]
    fn module(globals: &mut GlobalsBuilder) {
        /// A parameter bound to the frozen representation of a type that also has an unfrozen
        /// one.
        fn takes_frozen_record_type<'v>(
            x: FrozenValueTyped<'v, FrozenRecordType<'v>>,
        ) -> anyhow::Result<NoneType> {
            let _ = x;
            Ok(NoneType)
        }
    }

    fn assert() -> Assert<'static> {
        let mut a = Assert::new();
        a.globals_add(module);
        a
    }

    /// The declared type is the nominal one, so the unfrozen representation is of the type and
    /// fails as a conversion, not as a type mismatch.
    #[test]
    fn test_frozen_binding() {
        let mut a = assert();
        a.module("rec.star", "R = record(a=int)");
        a.pass("load('rec.star', 'R')\ntakes_frozen_record_type(R)");
        a.fail(
            "takes_frozen_record_type(record(a=int))",
            "Expected frozen value of type",
        );
    }
}
