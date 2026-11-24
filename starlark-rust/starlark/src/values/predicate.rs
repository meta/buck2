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

//! Predicate types: a Starlark type that is a set of nominal types, used through a handle that
//! is a Rust trait object.
//!
//! "A value that can be put on a command line" is one: a string is, an artifact is, and what
//! takes one wants `&dyn CommandLineArgLike`, whichever of them it is. A predicate type has a
//! name in Starlark, is not a union (its members are not listed anywhere, and a crate
//! downstream can add one), and is declared once, with
//! [`starlark_predicate_type!`](crate::starlark_predicate_type):
//!
//! ```ignore
//! starlark_predicate_type! {
//!     /// A value that can be put on a command line.
//!     pub StarlarkCmdArgsLike<'v>;
//!     name "CmdArgsLike";
//!     get &'v dyn CommandLineArgLike<'v>;
//!     upstream_member StarlarkStr;
//! }
//! ```
//!
//! `StarlarkCmdArgsLike` is then a [`StarlarkTypeExpr`] whose [`Get`](StarlarkTypeExpr::Get)
//! is the trait object, so `ValueOf<'v, StarlarkCmdArgsLike>` is a parameter, a field and a
//! handle, and `StarlarkCmdArgsLike::global()` is the value that names the type in Starlark.
//!
//! A type is a member by a conversion from its handle to the predicate's, written in its own
//! `#[starlark_value]` block ([`StarlarkType::provide_predicate_handle`] shows one). A type
//! that cannot name the trait, because it is defined upstream of it, is named in the
//! declaration instead, as `StarlarkStr` is above, in the crate that implements the trait for
//! it. Either way a membership is declared where the trait is implemented, once.
//!
//! [`StarlarkType::provide_predicate_handle`]: crate::values::StarlarkType::provide_predicate_handle

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;

use pagable::StaticValue;

use crate::typing::Ty;
use crate::typing::TyStarlarkValue;
use crate::typing::predicate::TyPredicate;
use crate::values::StarlarkTypeExpr;
use crate::values::Value;
use crate::values::demand::request_predicate_handle;

/// A predicate type as data: what its declaration says, for the typechecker, the runtime
/// matcher and the documentation. There is one per predicate type, and it is compared by name.
pub struct PredicateInfo {
    name: &'static str,
    docs: &'static str,
    matches: for<'v> fn(Value<'v>) -> bool,
    upstream_members: fn() -> Vec<TyStarlarkValue>,
}

/// A predicate type as data.
pub type Predicate = StaticValue<PredicateInfo>;

pagable::declare_static_value_type!(PredicateInfo, PredicateInfoStaticEntry);

impl PredicateInfo {
    /// What [`starlark_predicate_type!`](crate::starlark_predicate_type) says of the type.
    #[doc(hidden)]
    pub const fn new(
        name: &'static str,
        docs: &'static str,
        matches: for<'v> fn(Value<'v>) -> bool,
        upstream_members: fn() -> Vec<TyStarlarkValue>,
    ) -> PredicateInfo {
        PredicateInfo {
            name,
            docs,
            matches,
            upstream_members,
        }
    }

    /// The name of the type in Starlark.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The documentation of the type, as written on its declaration.
    pub fn docs(&self) -> &'static str {
        self.docs
    }

    /// Whether `value` is of the type.
    #[inline]
    pub fn matches(&self, value: Value) -> bool {
        (self.matches)(value)
    }

    /// The members that the declaration names because they cannot say so themselves.
    pub(crate) fn upstream_members(&self) -> Vec<TyStarlarkValue> {
        (self.upstream_members)()
    }
}

impl Debug for PredicateInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PredicateInfo")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Display for PredicateInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

impl PartialEq for PredicateInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for PredicateInfo {}

impl Hash for PredicateInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state)
    }
}

impl PartialOrd for PredicateInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PredicateInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(other.name)
    }
}

/// A predicate type. [`starlark_predicate_type!`](crate::starlark_predicate_type) implements
/// this for the marker it declares, together with [`StarlarkTypeExpr`]; there is nothing to
/// write by hand.
pub trait StarlarkPredicate: for<'v> StarlarkTypeExpr<'v> + 'static {
    /// The type as data.
    const PREDICATE: Predicate;

    /// The handle to `value` if it is one of the members named in the declaration.
    fn upstream_handle<'v>(value: Value<'v>) -> Option<<Self as StarlarkTypeExpr<'v>>::Get>;
}

/// The `Ty` of the predicate type `P`: a named type that is not a union.
pub fn predicate_ty<P: StarlarkPredicate>() -> Ty {
    Ty::custom(TyPredicate::new(P::PREDICATE))
}

/// The handle of the predicate type `P` to `value`, if `value` is of it: the members named in
/// the declaration first, in the order written there, then what the type of `value` says in
/// its own block.
///
/// This is the one check of the type: `P`'s [`get`](StarlarkTypeExpr::get) is this, and the
/// matcher of its `Ty` asks `get`.
#[inline]
pub fn predicate_handle<'v, P: StarlarkPredicate>(
    value: Value<'v>,
) -> Option<<P as StarlarkTypeExpr<'v>>::Get> {
    match P::upstream_handle(value) {
        Some(handle) => Some(handle),
        None => request_predicate_handle::<P>(value),
    }
}

/// Declares a [predicate type](crate::values::predicate).
///
/// ```ignore
/// starlark_predicate_type! {
///     /// A value that can be put on a command line.
///     pub StarlarkCmdArgsLike<'v>;
///     name "CmdArgsLike";
///     get &'v dyn CommandLineArgLike<'v>;
///     upstream_member StarlarkStr;
///     upstream_member StarlarkTargetLabel;
/// }
/// ```
///
/// The statements, in this order:
///
/// - `pub StarlarkCmdArgsLike<'v>;` is the marker: the Rust type that stands for the predicate
///   type where a type is written, as in `ValueOf<'v, StarlarkCmdArgsLike>`. Its doc comment
///   is the documentation of the type in Starlark, and its lifetime is the one the handle is
///   written with.
/// - `name` is the name of the type in Starlark: what a signature and an error message print,
///   and the name to set the global under.
/// - `get` is the handle, what a member is used through: the marker's
///   [`Get`](crate::values::StarlarkTypeExpr::Get), and what a `ValueOf` of the marker derefs
///   to the target of.
/// - `upstream_member`, any number of times, names a member that cannot say so in its own
///   `#[starlark_value]` block, because it is defined upstream of the handle's trait and so
///   cannot name it. It is named by a representation that implements the trait, in the crate
///   that implements the trait for it. These are not the members of the type: every other
///   member says that it is one in its own block, with `#[starlark(implements = ...)]`. A
///   value is tested against the upstream members first, in the order written.
///
/// This declares the marker as a [`StarlarkTypeExpr`](crate::values::StarlarkTypeExpr) whose
/// `Get` is the handle, makes the handle a parameter binding and a payload of a
/// `#[derive(UnpackValue)]` union, and gives the marker a function `global()`, the value that
/// stands for the type in Starlark, to set in a `GlobalsBuilder` under the name.
#[macro_export]
macro_rules! starlark_predicate_type {
    (
        $(#[doc = $docs:literal])*
        $vis:vis $marker:ident<$lt:lifetime>;
        name $name:literal;
        get $handle:ty;
        $(upstream_member $upstream:ty;)*
    ) => {
        $(#[doc = $docs])*
        $vis struct $marker;

        impl<$lt> $crate::values::StarlarkTypeExpr<$lt> for $marker {
            type Get = $handle;

            fn starlark_type_repr() -> $crate::typing::Ty {
                $crate::values::predicate::predicate_ty::<$marker>()
            }

            #[inline]
            fn get(value: $crate::values::Value<$lt>) -> ::std::option::Option<$handle> {
                $crate::values::predicate::predicate_handle::<$marker>(value)
            }
        }

        impl<$lt> $crate::values::StarlarkTypeExprStable<$lt> for $marker {}

        // SAFETY: the marker has no lifetime.
        unsafe impl<$lt> $crate::any::ProvidesStaticType<$lt> for $marker {
            type StaticType = $marker;
        }

        impl $crate::any::IsStaticType for $marker {
            type Reinfect<'lt> = $marker;
        }

        impl $crate::values::predicate::StarlarkPredicate for $marker {
            const PREDICATE: $crate::values::predicate::Predicate = {
                $crate::__derive_refs::static_value!(
                    PREDICATE: $crate::values::predicate::PredicateInfo = &$crate::values::predicate::PredicateInfo::new(
                        $name,
                        ::std::concat!($($docs, "\n"),*),
                        |value| <$marker as $crate::values::StarlarkTypeExpr>::get(value).is_some(),
                        || ::std::vec![$($crate::typing::TyStarlarkValue::new::<$upstream>()),*],
                    ),
                    $crate::values::predicate::PredicateInfoStaticEntry
                );
                PREDICATE
            };

            #[inline]
            fn upstream_handle<$lt>(
                value: $crate::values::Value<$lt>,
            ) -> ::std::option::Option<$handle> {
                $(
                    if let ::std::option::Option::Some(x) = value.downcast_ref::<$upstream>() {
                        return ::std::option::Option::Some(x);
                    }
                )*
                let _ = value;
                ::std::option::Option::None
            }
        }

        impl<$lt> $crate::values::DerefHandle<$lt> for $marker {
            type Target = <$handle as ::std::ops::Deref>::Target;

            #[inline]
            fn deref_handle(value: $crate::values::ValueOf<$lt, $marker>) -> $handle {
                value.get()
            }
        }

        impl<$lt> $crate::values::UnpackValue<$lt> for $handle {
            type Type = $marker;
            type Error = ::std::convert::Infallible;

            #[inline]
            fn unpack(
                _value: $crate::values::ValueOf<$lt, $marker>,
                get: $handle,
            ) -> ::std::result::Result<Self, ::std::convert::Infallible> {
                ::std::result::Result::Ok(get)
            }
        }

        impl<$lt> $crate::values::type_expr::UnionPayload<$lt> for $handle {
            type Expr = $marker;

            #[inline]
            fn from_value(value: $crate::values::Value<$lt>) -> ::std::option::Option<Self> {
                <$marker as $crate::values::StarlarkTypeExpr<$lt>>::get(value)
            }
        }

        const _: () = {
            $crate::declare_starlark_value_as_type!(TYPE, $marker, new_predicate);

            impl $marker {
                /// The value that stands for this type in Starlark.
                // Not every predicate type is given a global.
                #[allow(dead_code)]
                $vis fn global() -> $crate::values::starlark_value_as_type::StarlarkValueAsType<$marker> {
                    TYPE
                }
            }
        };
    };
}
