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
use crate::values::Value;
use crate::values::ValueOf;
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

/// Debug-build check that an unpack agreed with the matcher for its declared type: the value was
/// accepted, meaning unpacked or rejected only by a conversion error, iff it is of the type.
///
/// This is the contract of [`UnpackValue`]; the typechecker and the generated documentation take
/// it on faith, and nothing else verifies it. A disagreement is a bug in the `UnpackValue` or
/// `StarlarkTypeRepr` impl rather than in the Starlark program, hence a panic.
#[cfg(debug_assertions)]
fn check_type_agreement<'v, T: UnpackValue<'v>>(
    value: Value<'v>,
    accepted: bool,
    param_name: Option<&str>,
) {
    use crate::values::typing::type_compiled::matcher::TypeMatcher;

    let rust_type = std::any::type_name::<T>();
    let matcher = type_matcher::<T>();
    let matches = matcher.matches(value);
    if matches != accepted {
        let param = match param_name {
            Some(name) => format!("parameter `{name}`"),
            None => "value".to_owned(),
        };
        panic!(
            "Unpack of {param} disagrees with its declared type: Rust type `{rust_type}` declares \
             Starlark type `{ty}`, which {matches} `{value}`, but the unpack {accepted} it",
            ty = T::starlark_type_repr(),
            matches = if matches { "accepts" } else { "rejects" },
            value = value.to_string_for_type_error(),
            accepted = if accepted { "accepted" } else { "rejected" },
        );
    }
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

/// How to convert a [`Value`] to a Rust type. Required for all arguments in
/// a [`#[starlark_module]`](macro@crate::starlark_module) definition.
///
/// Note for simple references it often can be implemented with `#[starlark_value(UnpackValue)]`,
/// for example:
///
/// ```
/// # use allocative::Allocative;
/// # use starlark::any::ProvidesStaticType;
/// # use starlark::values::{NoSerialize, StarlarkPagable, StarlarkValue, starlark_value};
///
/// #[derive(
///     Debug,
///     derive_more::Display,
///     Allocative,
///     NoSerialize,
///     StarlarkPagable,
///     ProvidesStaticType
/// )]
/// struct MySimpleValue;
///
/// #[starlark_value(type = "MySimpleValue", UnpackValue, StarlarkTypeRepr)]
/// impl<'v> StarlarkValue<'v> for MySimpleValue {}
/// ```
///
/// Whereas for types that aren't also [`StarlarkValue`](crate::values::StarlarkValue) you can define:
///
/// ```
/// # use either::Either;
/// # use starlark::typing::Ty;
/// # use starlark::values::{UnpackValue, Value};
/// # use starlark::values::type_repr::StarlarkTypeRepr;
///
/// struct BoolOrInt(i32);
///
/// impl StarlarkTypeRepr for BoolOrInt {
///     type Canonical = <Either<bool, i32> as StarlarkTypeRepr>::Canonical;
///
///     fn starlark_type_repr() -> Ty {
///         Either::<bool, i32>::starlark_type_repr()
///     }
/// }
///
/// impl<'v> UnpackValue<'v> for BoolOrInt {
///     type Error = starlark::Error;
///
///     fn unpack_value_impl(value: Value<'v>) -> starlark::Result<Option<Self>> {
///         if let Some(x) = value.unpack_bool() {
///             Ok(Some(BoolOrInt(x as i32)))
///         } else {
///             let Some(x) = i32::unpack_value(value)? else {
///                 return Ok(None);
///             };
///             Ok(Some(BoolOrInt(x)))
///         }
///     }
/// }
/// ```
pub trait UnpackValue<'v>: Sized + StarlarkTypeRepr {
    /// Error returned when type matches, but conversion fails.
    ///
    /// Typically [`starlark::Error`](crate::Error), [`anyhow::Error`], or [`Infallible`].
    type Error: UnpackValueError;

    /// Given a [`Value`], try and unpack it into the given type,
    /// which may involve some element of conversion.
    ///
    /// Return `None` if the value is not of expected type (as described by [`StarlarkTypeRepr`],
    /// and return `Err` if the value is of expected type, but conversion cannot be performed.
    /// For example, when unpacking an integer to `String`, return `None`,
    /// and when unpacking a large integer to `i32`, return `Err`.
    ///
    /// This function is needs to be implemented, but usually not meant to be called directly.
    /// Consider using [`unpack_value`](UnpackValue::unpack_value),
    /// [`unpack_value_err`](UnpackValue::unpack_value_err),
    /// [`unpack_value_opt`](UnpackValue::unpack_value_opt) instead.
    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error>;

    /// Given a [`Value`], try and unpack it into the given type,
    /// which may involve some element of conversion.
    ///
    /// Return `None` if the value is not of expected type (as described by [`StarlarkTypeRepr`],
    /// and return `Err` if the value is of expected type, but conversion cannot be performed.
    /// For example, when unpacking an integer to `String`, return `None`,
    /// and when unpacking a large integer to `i32`, return `Err`.
    fn unpack_value(value: Value<'v>) -> Result<Option<Self>, crate::Error> {
        Self::unpack_value_impl(value).map_err(Self::Error::into_error)
    }

    /// Unpack a value if unpacking is infallible.
    fn unpack_value_opt(value: Value<'v>) -> Option<Self>
    where
        Self::Error: UnpackValueErrorInfallible,
    {
        match Self::unpack_value_impl(value) {
            Ok(x) => x,
            Err(e) => Self::Error::into_infallible(e),
        }
    }

    /// Unpack a value, but return error instead of `None` if unpacking fails.
    #[inline]
    fn unpack_value_err(value: Value<'v>) -> crate::Result<Self> {
        let unpacked = Self::unpack_value(value);
        #[cfg(debug_assertions)]
        check_type_agreement::<Self>(value, !matches!(unpacked, Ok(None)), None);
        unpacked?.ok_or_else(|| expected_type_error(value, Self::starlark_type_repr))
    }

    /// Unpack value, but instead of `None` return error about incorrect argument type.
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

        let unpacked = Self::unpack_value(value);
        #[cfg(debug_assertions)]
        check_type_agreement::<Self>(value, !matches!(unpacked, Ok(None)), None);
        unpacked?.ok_or_else(|| error(value, Self::starlark_type_repr))
    }

    /// Unpack value, but instead of `None` return error about incorrect named argument type.
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

        let unpacked = Self::unpack_value(value);
        #[cfg(debug_assertions)]
        check_type_agreement::<Self>(value, !matches!(unpacked, Ok(None)), Some(param_name));
        unpacked
            .into_anyhow_result()
            .with_context(|| {
                format!(
                    "Error unpacking value for parameter `{}` of type `{}",
                    param_name,
                    Self::starlark_type_repr()
                )
            })?
            .ok_or_else(|| error(value, param_name, Self::starlark_type_repr))
    }
}

impl<'v> UnpackValue<'v> for Value<'v> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(Some(value))
    }
}

impl<'v, TLeft: UnpackValue<'v>, TRight: UnpackValue<'v>> UnpackValue<'v>
    for Either<TLeft, TRight>
{
    type Error = Either<TLeft::Error, TRight::Error>;

    // Only implemented for types that implement [`UnpackValue`]. Nonsensical for other types.
    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        match TLeft::unpack_value_impl(value).map_err(Either::Left)? {
            Some(left) => Ok(Some(Self::Left(left))),
            _ => Ok(TRight::unpack_value_impl(value)
                .map_err(Either::Right)?
                .map(Self::Right)),
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

#[cfg(all(test, debug_assertions))]
mod tests {
    use std::convert::Infallible;

    use starlark_derive::starlark_module;

    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::typing::Ty;
    use crate::values::UnpackValue;
    use crate::values::Value;
    use crate::values::ValueTyped;
    use crate::values::none::NoneType;
    use crate::values::record::record_type::FrozenRecordType;
    use crate::values::type_repr::StarlarkTypeRepr;

    /// Declares `str` but also accepts anything else.
    struct LooseStr;

    impl StarlarkTypeRepr for LooseStr {
        type Canonical = <String as StarlarkTypeRepr>::Canonical;

        fn starlark_type_repr() -> Ty {
            String::starlark_type_repr()
        }
    }

    impl<'v> UnpackValue<'v> for LooseStr {
        type Error = Infallible;

        fn unpack_value_impl(_value: Value<'v>) -> Result<Option<Self>, Self::Error> {
            Ok(Some(LooseStr))
        }
    }

    /// Declares `str` but rejects the empty string.
    struct StrictStr;

    impl StarlarkTypeRepr for StrictStr {
        type Canonical = <String as StarlarkTypeRepr>::Canonical;

        fn starlark_type_repr() -> Ty {
            String::starlark_type_repr()
        }
    }

    impl<'v> UnpackValue<'v> for StrictStr {
        type Error = Infallible;

        fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
            Ok(match value.unpack_str() {
                Some(s) if !s.is_empty() => Some(StrictStr),
                _ => None,
            })
        }
    }

    #[starlark_module]
    fn module(globals: &mut GlobalsBuilder) {
        fn takes_loose_str(x: LooseStr) -> anyhow::Result<NoneType> {
            let _ = x;
            Ok(NoneType)
        }

        fn takes_strict_str(x: StrictStr) -> anyhow::Result<NoneType> {
            let _ = x;
            Ok(NoneType)
        }

        /// A parameter bound to the frozen representation of a type that also has an unfrozen
        /// one.
        fn takes_frozen_record_type<'v>(
            x: ValueTyped<'v, FrozenRecordType<'v>>,
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

    #[test]
    fn test_agreeing_unpacks_pass() {
        let a = assert();
        a.pass("takes_loose_str('x')");
        a.pass("takes_strict_str('x')");
    }

    #[test]
    #[should_panic(
        expected = "declares Starlark type `str`, which rejects `int (repr: 1)`, but the unpack accepted it"
    )]
    fn test_looser_than_declared_type() {
        assert().pass("takes_loose_str(1)");
    }

    #[test]
    #[should_panic(
        expected = "declares Starlark type `str`, which accepts `string (repr: \"\")`, but the unpack rejected it"
    )]
    fn test_stricter_than_declared_type() {
        assert().fail("takes_strict_str('')", "");
    }

    #[test]
    #[should_panic(
        expected = "which accepts `function (repr: record(a=field(int)))`, but the unpack rejected it"
    )]
    fn test_representation_specific_binding() {
        assert().fail("takes_frozen_record_type(record(a=int))", "");
    }
}
