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

//! The `Ty` of a [recursive type](crate::values::recursive_type).

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark_derive::type_matcher;
use starlark_syntax::codemap::Span;

use crate as starlark;
use crate::typing::Ty;
use crate::typing::TyBasic;
use crate::typing::TypingBinOp;
use crate::typing::TypingOracleCtx;
use crate::typing::call_args::TyCallArgs;
use crate::typing::callable::TyCallable;
use crate::typing::custom::TyCustomDyn;
use crate::typing::custom::TyCustomImpl;
use crate::typing::error::InternalError;
use crate::typing::error::TypingNoContextError;
use crate::typing::error::TypingNoContextOrInternalError;
use crate::typing::error::TypingOrInternalError;
use crate::values::Value;
use crate::values::recursive_type::RecursiveType;
use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
use crate::values::typing::type_compiled::matcher::TypeMatcher;
use crate::values::typing::type_compiled::matcher::TypeMatcherDyn;

/// Matches the values of a recursive type, by the check its `get` is.
#[derive(Allocative, Debug, Clone, Pagable)]
#[pagable_typetag(TypeMatcherDyn)]
struct IsRecursive {
    recursive: RecursiveType,
}

#[type_matcher]
impl TypeMatcher for IsRecursive {
    fn matches(&self, value: Value) -> bool {
        self.recursive.matches(value)
    }
}

/// A recursive type: a named type that stands for a type expression which mentions it.
///
/// It is not the expression: it prints as its name, and it is unfolded to the expression only
/// where it is compared with another type, one step at a time. What is done with a value of
/// the type is not checked. There is one recursive type per name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Allocative, Pagable)]
#[pagable_typetag(TyCustomDyn)]
pub(crate) struct TyRecursive {
    recursive: RecursiveType,
}

impl PartialOrd for TyRecursive {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TyRecursive {
    fn cmp(&self, other: &Self) -> Ordering {
        self.recursive.value().cmp(other.recursive.value())
    }
}

impl TyRecursive {
    pub(crate) fn new(recursive: RecursiveType) -> TyRecursive {
        TyRecursive { recursive }
    }

    pub(crate) fn recursive(&self) -> RecursiveType {
        self.recursive
    }
}

impl Display for TyRecursive {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.recursive.name())
    }
}

impl TyCustomImpl for TyRecursive {
    fn as_name(&self) -> Option<&str> {
        Some(self.recursive.name())
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

    fn intersects_with(
        &self,
        other: &TyBasic,
        ctx: TypingOracleCtx,
    ) -> Result<bool, InternalError> {
        // Another recursive type is "maybe": two names could unfold into each other forever.
        if let TyBasic::Custom(other) = other {
            if other.0.as_any().is::<TyRecursive>() {
                return Ok(true);
            }
        }
        // The type takes what its expression takes. The expression mentions the type only
        // under a container, and each step into a container looks at an item type of `other`,
        // so this ends.
        ctx.intersects(&self.recursive.unfolded(), &Ty::basic(other.dupe()))
    }

    fn matcher<T: TypeMatcherAlloc>(&self, factory: T) -> T::Result {
        factory.alloc(IsRecursive {
            recursive: self.recursive,
        })
    }
}
