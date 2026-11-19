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

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use dupe::Dupe;
use mini_map::MiniFixedSortedMap;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark_derive::type_matcher;

use crate as starlark;
use crate::codemap::Span;
use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::call_args::TyCallArgs;
use crate::typing::callable::TyCallable;
use crate::typing::custom::TyCustomDyn;
use crate::typing::custom::TyCustomImpl;
use crate::typing::error::TypingNoContextError;
use crate::typing::error::TypingOrInternalError;
use crate::typing::function::TyCustomFunctionImpl;
use crate::typing::oracle::ctx::TypingOracleCtx;
use crate::util::arc_str::ArcStr;
use crate::values::Value;
use crate::values::starlark_type_id::StarlarkTypeId;
use crate::values::types::namespace::value::Namespace;
use crate::values::typing::type_compiled::alloc::TypeMatcherAlloc;
use crate::values::typing::type_compiled::matcher::TypeMatcher;
use crate::values::typing::type_compiled::matcher::TypeMatcherDyn;

#[derive(Allocative, Eq, PartialEq, Hash, Debug, Clone, Copy, Dupe, Pagable)]
#[pagable_typetag(TypeMatcherDyn)]
struct NamespaceMatcher;

#[type_matcher]
impl TypeMatcher for NamespaceMatcher {
    fn matches(&self, value: Value) -> bool {
        value.starlark_type_id() == StarlarkTypeId::of::<Namespace<'static>>()
    }
}

#[derive(
    Allocative, Clone, Copy, Dupe, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Pagable
)]
pub(super) struct TyNamespaceFunction;

impl TyCustomFunctionImpl for TyNamespaceFunction {
    fn is_type(&self) -> bool {
        // `namespace` is declared `as_type = Namespace`, so at runtime it has a
        // `.type` attribute and can be used in a type expression (`namespace | None`).
        true
    }

    fn as_callable(&self) -> TyCallable {
        // TODO(nga): this should be obtained from function signature from function definition.
        TyCallable::new(
            ParamSpec::kwargs(Ty::any()),
            Ty::custom(TyNamespace {
                fields: Default::default(),
                extra: true,
            }),
        )
    }

    fn validate_call(
        &self,
        _span: Span,
        args: &TyCallArgs,
        oracle: TypingOracleCtx,
    ) -> Result<Ty, TypingOrInternalError> {
        if let [pos, ..] = args.pos.as_slice() {
            return Err(oracle.msg_error(pos.span, "Positional arguments not allowed"));
        }
        let mut fields = Vec::new();
        for named in &args.named {
            let (name, ty) = &named.node;
            fields.push((ArcStr::from(*name), ty.clone()));
        }
        let extra = args.kwargs.is_some();
        Ok(Ty::custom(TyNamespace {
            fields: MiniFixedSortedMap::from_iter(fields),
            extra,
        }))
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Allocative, Pagable
)]
#[pagable_typetag(TyCustomDyn)]
pub(super) struct TyNamespace {
    pub(super) fields: MiniFixedSortedMap<ArcStr, Ty>,
    /// [`true`] if there might be additional fields not captured above,
    /// [`false`] if this struct has no extra members.
    pub(super) extra: bool,
}

impl TyCustomImpl for TyNamespace {
    fn as_name(&self) -> Option<&str> {
        Some("namespace")
    }

    fn attribute(&self, attr: &str) -> Result<Ty, TypingNoContextError> {
        match self.fields.get(attr) {
            Some(ty) => Ok(ty.dupe()),
            None => {
                if self.extra {
                    Ok(Ty::any())
                } else {
                    Err(TypingNoContextError)
                }
            }
        }
    }

    fn matcher<T: TypeMatcherAlloc>(&self, factory: T) -> T::Result {
        factory.alloc(NamespaceMatcher)
    }
}

impl Display for TyNamespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let TyNamespace { fields, extra } = self;
        display_container::fmt_container(
            f,
            "namespace(",
            ")",
            display_container::iter_display_chain(
                fields.iter().map(|(k, v)| format!("{k} = {v}")),
                extra.then_some(".."),
            ),
        )
    }
}
