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

//! The `Ty` of a [predicate type](crate::values::predicate).

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark_derive::type_matcher;
use starlark_syntax::codemap::Span;

use crate as starlark;
use crate::typing::Ty;
use crate::typing::TyBasic;
use crate::typing::TyStarlarkValue;
use crate::typing::TyUser;
use crate::typing::TypingBinOp;
use crate::typing::TypingOracleCtx;
use crate::typing::call_args::TyCallArgs;
use crate::typing::callable::TyCallable;
use crate::typing::custom::TyCustomDyn;
use crate::typing::custom::TyCustomImpl;
use crate::typing::error::TypingNoContextError;
use crate::typing::error::TypingNoContextOrInternalError;
use crate::typing::error::TypingOrInternalError;
use crate::values::Value;
use crate::values::predicate::Predicate;
use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
use crate::values::typing::type_compiled::matcher::TypeMatcher;
use crate::values::typing::type_compiled::matcher::TypeMatcherDyn;

/// Matches the values of a predicate type, by the check its `get` is.
#[derive(Allocative, Debug, Clone, Pagable)]
#[pagable_typetag(TypeMatcherDyn)]
struct IsPredicate {
    predicate: Predicate,
}

#[type_matcher]
impl TypeMatcher for IsPredicate {
    fn matches(&self, value: Value) -> bool {
        self.predicate.matches(value)
    }
}

/// A predicate type: a named type whose members are the nominal types that say so, or that its
/// declaration names.
///
/// It is not a union. The members are not enumerable, since a crate downstream of the
/// declaration can add one, so the type answers for one type at a time, and says "maybe" where
/// the answer would need the whole set: an operation on a value of the type is not checked,
/// and two predicate types intersect.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Allocative, Pagable)]
#[pagable_typetag(TyCustomDyn)]
pub(crate) struct TyPredicate {
    predicate: Predicate,
}

impl PartialOrd for TyPredicate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TyPredicate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.predicate.value().cmp(other.predicate.value())
    }
}

impl TyPredicate {
    pub(crate) fn new(predicate: Predicate) -> TyPredicate {
        TyPredicate { predicate }
    }

    /// Whether the nominal type is a member: its block says so, or the declaration names it.
    fn has_member(&self, ty: TyStarlarkValue) -> bool {
        ty.implemented_predicates().contains(&self.predicate)
            || self.predicate.upstream_members().contains(&ty)
    }
}

impl Display for TyPredicate {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.predicate.name())
    }
}

impl TyCustomImpl for TyPredicate {
    fn as_name(&self) -> Option<&str> {
        Some(self.predicate.name())
    }

    fn validate_call(
        &self,
        _span: Span,
        _args: &TyCallArgs,
        _oracle: TypingOracleCtx,
    ) -> Result<Ty, TypingOrInternalError> {
        Ok(Ty::any())
    }

    fn as_callable(&self) -> Option<TyCallable> {
        Some(TyCallable::any())
    }

    fn bin_op(
        &self,
        _bin_op: TypingBinOp,
        _rhs: &TyBasic,
        _ctx: &TypingOracleCtx,
    ) -> Result<Ty, TypingNoContextOrInternalError> {
        Ok(Ty::any())
    }

    fn iter_item(&self) -> Result<Ty, TypingNoContextError> {
        Ok(Ty::any())
    }

    fn index(
        &self,
        _item: &TyBasic,
        _ctx: &TypingOracleCtx,
    ) -> Result<Ty, TypingNoContextOrInternalError> {
        Ok(Ty::any())
    }

    fn attribute(&self, _attr: &str) -> Result<Ty, TypingNoContextError> {
        Ok(Ty::any())
    }

    fn intersects_with(&self, other: &TyBasic) -> bool {
        match other {
            TyBasic::StarlarkValue(other) => self.has_member(*other),
            TyBasic::Custom(other) => match other.0.as_any().downcast_ref::<TyUser>() {
                // A `TyUser` that names this type as a supertype says so itself. One that is
                // made of a member's values is a member, and an abstract one, whose values are
                // not its base's, may hold a member's. The abstract rule is for `Provider`,
                // and goes when `Provider` is a predicate type itself.
                Some(other) => other.is_abstract() || self.has_member(other.base()),
                None => false,
            },
            _ => false,
        }
    }

    fn matcher<T: TypeMatcherAlloc>(&self, factory: T) -> T::Result {
        factory.alloc(IsPredicate {
            predicate: self.predicate,
        })
    }
}
