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

//! Recursive types: a Starlark type that is a name for a type expression which mentions the
//! name.
//!
//! "What `cmd_args` takes" is one: a value that can be put on a command line, or a list of what
//! `cmd_args` takes. No type expression says that, since it would have to contain itself, so
//! the type is declared under a name, with
//! [`starlark_recursive_type!`](crate::starlark_recursive_type), and the expression mentions
//! the name.

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;

use pagable::StaticValue;

use crate::typing::Ty;
use crate::typing::recursive::TyRecursive;
use crate::values::StarlarkTypeExpr;
use crate::values::Value;

/// A recursive type as data: what its declaration says, for the typechecker, the runtime
/// matcher and the documentation. There is one per recursive type, and it is compared by name.
pub struct RecursiveTypeInfo {
    name: &'static str,
    docs: &'static str,
    matches: for<'v> fn(Value<'v>) -> bool,
    unfolded: fn() -> Ty,
}

/// A recursive type as data.
pub type RecursiveType = StaticValue<RecursiveTypeInfo>;

pagable::declare_static_value_type!(RecursiveTypeInfo, RecursiveTypeInfoStaticEntry);

impl RecursiveTypeInfo {
    /// What [`starlark_recursive_type!`](crate::starlark_recursive_type) says of the type.
    #[doc(hidden)]
    pub const fn new(
        name: &'static str,
        docs: &'static str,
        matches: for<'v> fn(Value<'v>) -> bool,
        unfolded: fn() -> Ty,
    ) -> RecursiveTypeInfo {
        RecursiveTypeInfo {
            name,
            docs,
            matches,
            unfolded,
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

    /// The type expression the type is a name for. It mentions the type.
    pub(crate) fn unfolded(&self) -> Ty {
        (self.unfolded)()
    }
}

impl Debug for RecursiveTypeInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecursiveTypeInfo")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RecursiveTypeInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for RecursiveTypeInfo {}

impl Hash for RecursiveTypeInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state)
    }
}

impl PartialOrd for RecursiveTypeInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RecursiveTypeInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(other.name)
    }
}

/// A recursive type. [`starlark_recursive_type!`](crate::starlark_recursive_type) implements
/// this for the marker it declares, together with [`StarlarkTypeExpr`]; there is nothing to
/// write by hand.
pub trait StarlarkRecursiveType: for<'v> StarlarkTypeExpr<'v> + 'static {
    /// The type as data.
    const RECURSIVE_TYPE: RecursiveType;
}

/// The `Ty` of the recursive type `T`: a named type, which the typechecker unfolds to its
/// expression where it compares it with another type.
pub fn recursive_ty<T: StarlarkRecursiveType>() -> Ty {
    Ty::custom(TyRecursive::new(T::RECURSIVE_TYPE))
}

/// Declares a [recursive type](crate::values::recursive_type): a name for a type expression
/// that mentions the name.
///
/// ```ignore
/// starlark_recursive_type! {
///     /// What `cmd_args` takes: a value that can be put on a command line, or a list of these.
///     pub StarlarkCmdArgsInput<'v>;
///     name "CmdArgsInput";
///     is Either<StarlarkCmdArgsLike, ListType<StarlarkCmdArgsInput>>;
/// }
/// ```
///
/// The statements, in this order:
///
/// - `pub StarlarkCmdArgsInput<'v>;` is the marker: the Rust type that stands for the type
///   where a type is written, as in `ValueOf<'v, StarlarkCmdArgsInput>`. Its doc comment is the
///   documentation of the type in Starlark, and its lifetime is the one the expression is
///   written with.
/// - `name` is the name of the type in Starlark: what a signature and an error message print,
///   and the name to set the global under.
/// - `is` is the type expression the name stands for, written with the marker where it mentions
///   the type.
///
/// # What the type is
///
/// The type is the least solution of `name = expression`: a value is of it when it is of the
/// expression, where a mention of the name is the type again, and that has been decided in
/// finitely many steps. The type above is a `CmdArgsLike`, a list of those, a list of those and
/// of lists of those, and so on to any depth. A list that contains itself is not of it.
///
/// The marker is a [`StarlarkTypeExpr`](crate::values::StarlarkTypeExpr) whose `Get` and `get`
/// are the expression's, so the check at run time is exactly that definition: `get` of the
/// expression asks the marker again for what a container holds. On a value that contains
/// itself the check does not return and overflows the stack. The type is as
/// [stable](crate::values::StarlarkTypeExprStable) as its expression, so one that looks into a
/// list is not, and a `ValueOf` of it is read with `try_get`. Nothing is a member of a
/// recursive type; a type whose members say so themselves is a
/// [predicate type](crate::values::predicate).
///
/// The typechecker knows the type by its name, which is what it prints. Where it compares the
/// type with another, it unfolds the name one step: the type intersects what the expression
/// intersects, and a mention of the name in the expression is again the type, to be unfolded
/// when a comparison gets to it. That is exact but for two things:
///
/// - Against another recursive type the answer is "maybe", since two names would otherwise
///   unfold into each other without end.
/// - What is done with a value of the type (an attribute, an operator, an index, a call,
///   iteration) is not checked. For that reason the type is also taken where a callable or an
///   iterable is wanted, whatever the expression.
///
/// # Where the expression may mention the name
///
/// Only under a container: `ListType`, `DictType`, `SetType`, `TupleType`. A mention anywhere
/// else, as in `Either<Foo, Name>` or `NoneOr<Name>`, would make the type contain itself, and
/// it does not compile: the marker's `Get` is the expression's, which would contain itself as
/// well.
///
/// The declaration gives the marker a function `global()`, the value that stands for the type
/// in Starlark, to set in a `GlobalsBuilder` under the name.
#[macro_export]
macro_rules! starlark_recursive_type {
    (
        $(#[doc = $docs:literal])*
        $vis:vis $marker:ident<$lt:lifetime>;
        name $name:literal;
        is $unfolded:ty;
    ) => {
        $(#[doc = $docs])*
        $vis struct $marker;

        impl<$lt> $crate::values::StarlarkTypeExpr<$lt> for $marker {
            type Get = <$unfolded as $crate::values::StarlarkTypeExpr<$lt>>::Get;

            fn starlark_type_repr() -> $crate::typing::Ty {
                $crate::values::recursive_type::recursive_ty::<$marker>()
            }

            #[inline]
            fn get(value: $crate::values::Value<$lt>) -> ::std::option::Option<Self::Get> {
                <$unfolded as $crate::values::StarlarkTypeExpr<$lt>>::get(value)
            }
        }

        // SAFETY: the marker has no lifetime.
        unsafe impl<$lt> $crate::any::ProvidesStaticType<$lt> for $marker {
            type StaticType = $marker;
        }

        impl $crate::any::IsStaticType for $marker {
            type Reinfect<'lt> = $marker;
        }

        impl $crate::values::recursive_type::StarlarkRecursiveType for $marker {
            const RECURSIVE_TYPE: $crate::values::recursive_type::RecursiveType = {
                // Functions with the marker's lifetime, which the expression may name.
                fn matches<$lt>(value: $crate::values::Value<$lt>) -> bool {
                    <$marker as $crate::values::StarlarkTypeExpr<$lt>>::get(value).is_some()
                }
                // `expect` would be unfulfilled in other crates, where clippy does not look
                // into this macro.
                #[allow(
                    clippy::extra_unused_lifetimes,
                    reason = "only the body names the lifetime"
                )]
                fn unfolded<$lt>() -> $crate::typing::Ty {
                    <$unfolded as $crate::values::StarlarkTypeExpr<$lt>>::starlark_type_repr()
                }
                $crate::__derive_refs::static_value!(
                    RECURSIVE_TYPE: $crate::values::recursive_type::RecursiveTypeInfo = &$crate::values::recursive_type::RecursiveTypeInfo::new(
                        $name,
                        ::std::concat!($($docs, "\n"),*),
                        matches,
                        unfolded,
                    ),
                    $crate::values::recursive_type::RecursiveTypeInfoStaticEntry
                );
                RECURSIVE_TYPE
            };
        }

        const _: () = {
            $crate::declare_starlark_value_as_type!(TYPE, $marker, new_recursive);

            impl $marker {
                /// The value that stands for this type in Starlark.
                $vis fn global() -> $crate::values::starlark_value_as_type::StarlarkValueAsType<$marker> {
                    TYPE
                }
            }
        };
    };
}
