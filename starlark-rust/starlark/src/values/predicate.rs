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
//! `#[starlark_value]` block:
//!
//! ```ignore
//! #[starlark(implements = StarlarkCmdArgsLike)]
//! fn cmd_args_like(this: ValueTyped<'v, Self>) -> &'v dyn CommandLineArgLike<'v> {
//!     this.as_ref()
//! }
//! ```
//!
//! The parameter is the type's [`Get`](crate::values::StarlarkType::Get): `ValueTyped<'v, Self>`
//! for a type with one representation, the view itself, by value, for a type with several. The
//! result is the predicate's handle, which lives as long as the heap, and that is why the
//! conversion starts from the handle and not from `&self`. A type is a member once, for every
//! representation.
//!
//! A type that cannot name the trait, because it is defined upstream of it, is named in the
//! declaration instead, as `StarlarkStr` is above, in the crate that implements the trait for
//! it. Either way a membership is declared where the trait is implemented, once.
//!
//! # The check
//!
//! "Is this value of the predicate type, and what is its handle" is [`predicate_handle`]. The
//! vtable of a representation lists the [`Membership`]s of its type, which is no entry for
//! most types and one for most of the rest, and a value is of the predicate type if the list
//! of its vtable has an entry for it. The entry is the conversion for that representation, so
//! the handle is one call away. The members named in the declaration are known by their
//! representation: a string before the list is read, since it is told by its tag, and the
//! others after it.

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::mem;
use std::ptr;

use pagable::StaticValue;

use crate::typing::Ty;
use crate::typing::TyStarlarkValue;
use crate::typing::predicate::TyPredicate;
use crate::values::StarlarkTypeExpr;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::string::StarlarkStr;

/// A predicate type as data: what its declaration says, for the typechecker, the runtime
/// matcher and the documentation. There is one per predicate type, a `static`.
///
/// Two are equal, and ordered, by name. The order has to be the same in every process: a `Ty`
/// that holds one is sorted, and one is written out as its place among all of them. Which
/// predicate type a [`Membership`] is in is another question, answered by which static it
/// is.
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

/// Whether the two are one predicate type: the same static.
#[inline]
pub(crate) fn is_predicate(a: Predicate, b: Predicate) -> bool {
    ptr::eq(a.0, b.0)
}

/// A predicate type. [`starlark_predicate_type!`](crate::starlark_predicate_type) implements
/// this for the marker it declares, together with [`StarlarkTypeExpr`]; there is nothing to
/// write by hand.
///
/// # Safety
///
/// [`PREDICATE`](Self::PREDICATE) is a `static` that is this type's alone: no other type's
/// `PREDICATE` is that static. A [`Membership`] is recognised as one in this type by it, and
/// its handle is then read as this type's [`Get`](StarlarkTypeExpr::Get).
pub unsafe trait StarlarkPredicate: for<'v> StarlarkTypeExpr<'v> + 'static {
    /// The type as data.
    const PREDICATE: Predicate;

    /// Whether the declaration names members: whether it has an `upstream_member`.
    const NAMES_MEMBERS: bool;

    /// The handle to `value` if it is one of the members named in the declaration.
    fn upstream_handle<'v>(value: Value<'v>) -> Option<<Self as StarlarkTypeExpr<'v>>::Get>;
}

/// The `Ty` of the predicate type `P`: a named type that is not a union.
pub fn predicate_ty<P: StarlarkPredicate>() -> Ty {
    Ty::custom(TyPredicate::new(P::PREDICATE))
}

/// Stands for the representation in the type of a [`Membership`] that a vtable holds: the
/// vtable is of one representation, and its type does not say which.
pub(crate) enum AnyRepresentation {}

/// A membership of a type in a predicate type, for one representation `R` of the type: the
/// predicate type, and the conversion from a value of the representation to the predicate
/// type's handle.
///
/// The vtable of a representation holds the memberships of its type, which
/// [`#[starlark_value]`](crate::values::starlark_value) makes from the conversions of the
/// type's block ([`Memberships`]). [`predicate_handle`] finds a value's membership there.
///
/// The parameter is the static type of the representation. It says whose values the
/// conversion takes, which is all that is left of the conversion's type here.
// `repr(C)`: the layout does not depend on the parameter, see `in_vtable`.
#[repr(C)]
pub struct Membership<R: ?Sized + 'static> {
    predicate: Predicate,
    /// A `fn(ValueTyped<'v, Rep>) -> <P as StarlarkTypeExpr<'v>>::Get`, where `Rep` is the
    /// representation whose static type is `R` and `P` is the predicate type of `predicate`.
    /// It is never called as the type written here.
    handle: unsafe fn(),
    representation: PhantomData<fn(&R)>,
}

impl<R: ?Sized + 'static> Membership<R> {
    /// The membership in `P` of the type that `Rep` is a representation of, with the
    /// conversion from a value of `Rep` to the handle of `P`.
    #[doc(hidden)]
    pub const fn new<'v, P, Rep>(
        handle: fn(ValueTyped<'v, Rep>) -> <P as StarlarkTypeExpr<'v>>::Get,
    ) -> Membership<R>
    where
        P: StarlarkPredicate,
        Rep: StarlarkValue<'v, StaticType = R>,
    {
        Membership {
            predicate: P::PREDICATE,
            // SAFETY: Function pointers are one size, and nothing calls this one before
            // `predicate_handle` has given it its type back.
            handle: unsafe {
                mem::transmute::<
                    fn(ValueTyped<'v, Rep>) -> <P as StarlarkTypeExpr<'v>>::Get,
                    unsafe fn(),
                >(handle)
            },
            representation: PhantomData,
        }
    }

    /// The predicate type this is a membership in.
    pub(crate) fn predicate(&self) -> Predicate {
        self.predicate
    }

    /// The memberships of a representation, as its vtable holds them.
    pub(crate) const fn in_vtable(
        memberships: &'static [Membership<R>],
    ) -> &'static [Membership<AnyRepresentation>] {
        // SAFETY: `R` is the parameter of a `PhantomData` and nothing else, so with `repr(C)`
        // the two types have one layout.
        unsafe {
            &*(memberships as *const [Membership<R>] as *const [Membership<AnyRepresentation>])
        }
    }
}

/// The memberships of a type in predicate types, as held by its representation `R`.
///
/// [`#[starlark_value]`](crate::values::starlark_value) implements this for every type, from
/// the functions of the type's block that are marked `#[starlark(implements = ..)]`: each
/// becomes one [`Membership`] per representation, which first makes the type's
/// [`Get`](crate::values::StarlarkType::Get) of the representation's handle
/// ([`get_typed`](StarlarkValue::get_typed)) and then is the function. There is nothing to
/// write by hand.
pub trait Memberships<'v, R: StarlarkValue<'v>> {
    /// The memberships, which the vtable of `R` holds.
    #[doc(hidden)]
    const MEMBERSHIPS: &'static [Membership<R::StaticType>] = &[];
}

// `predicate_handle` does not read the memberships of a string.
const _: () = assert!(<StarlarkStr as Memberships<'static, StarlarkStr>>::MEMBERSHIPS.is_empty());

/// The handle of the predicate type `P` to `value`, if `value` is of it: by the membership of
/// its type in `P`, or else as one of the members named in the declaration of `P`, tried in
/// the order written there.
///
/// This is the one check of the type: `P`'s [`get`](StarlarkTypeExpr::get) is this, and the
/// matcher of its `Ty` asks `get`.
#[inline]
pub fn predicate_handle<'v, P: StarlarkPredicate>(
    value: Value<'v>,
) -> Option<<P as StarlarkTypeExpr<'v>>::Get> {
    // No value is more often asked whether it is of a predicate type than a string. It is
    // told by its tag, without reading its vtable, and it has no memberships, so it is of the
    // type only if the declaration names it. Of the tests for the members named there, which
    // are inlined here, the one for a string is all that is left. A type whose declaration
    // names nobody does not look for strings.
    if P::NAMES_MEMBERS && value.is_str() {
        return P::upstream_handle(value);
    }
    for membership in value.get_ref().vtable().starlark_value.memberships {
        if is_predicate(membership.predicate, P::PREDICATE) {
            // SAFETY: Only `Membership::new` makes a membership, typed by the representation
            // whose values its function takes, and a vtable takes the `Memberships` of its
            // own representation. This one is in the vtable of `value`, so it was made as
            // `new::<Q, Rep>`, where `Rep` is the representation of `value` and
            // `Q::PREDICATE` is the static that `P::PREDICATE` is. Two predicate types do not
            // share that static, by the contract of `StarlarkPredicate`, so `Q` is `P`. The
            // function is then a `fn(ValueTyped<'a, Rep>) -> <P as StarlarkTypeExpr<'a>>::Get`,
            // and `ValueTyped` is a transparent `Value` that is a `Rep`, which `value` is.
            // The brand `'a` the function was written for is not the one of `value`, as for
            // every entry of a vtable (see `AValueVTable::new`).
            let handle = unsafe {
                mem::transmute::<unsafe fn(), fn(Value<'v>) -> <P as StarlarkTypeExpr<'v>>::Get>(
                    membership.handle,
                )
            };
            return Some(handle(value));
        }
    }
    if P::NAMES_MEMBERS {
        upstream_handle_out_of_line::<P>(value)
    } else {
        None
    }
}

/// [`StarlarkPredicate::upstream_handle`], as a call: where a value is neither a string nor
/// of a type that says it is a member, which is the rare case, the tests for the members
/// named in the declaration are not worth their size in every check.
#[inline(never)]
fn upstream_handle_out_of_line<'v, P: StarlarkPredicate>(
    value: Value<'v>,
) -> Option<<P as StarlarkTypeExpr<'v>>::Get> {
    P::upstream_handle(value)
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
///   value of a type that does not say so is tested against the upstream members, in the
///   order written.
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

        // SAFETY: the static is declared here, for this marker.
        unsafe impl $crate::values::predicate::StarlarkPredicate for $marker {
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

            const NAMES_MEMBERS: bool =
                !<[&str]>::is_empty(&[$(::std::stringify!($upstream)),*]);

            // `predicate_handle` says where this is inlined and where it is not.
            #[inline(always)]
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
