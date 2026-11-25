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

use std::sync::LazyLock;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;

use crate::codemap::Span;
use crate::codemap::Spanned;
use crate::environment::GlobalsBuilder;
use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::TyFunction;
use crate::typing::TypingOracleCtx;
use crate::typing::call_args::TyCallArgs;
use crate::typing::callable::TyCallable;
use crate::typing::error::TypingOrInternalError;
use crate::typing::function::TyCustomFunctionImpl;
use crate::values::list::StarlarkList;

#[derive(
    Allocative, Hash, Eq, PartialEq, Ord, PartialOrd, Clone, Debug, Pagable
)]
pub(crate) struct ListFunctionTy;

static LIST: LazyLock<TyFunction> = LazyLock::new(|| {
    TyFunction::new_with_type_attr(
        ParamSpec::pos_only([], [Ty::iter(Ty::any())]),
        Ty::any_list(),
        Ty::any_list(),
    )
});

impl TyCustomFunctionImpl for ListFunctionTy {
    fn is_type(&self) -> bool {
        true
    }

    fn as_callable(&self) -> TyCallable {
        LIST.callable.dupe()
    }

    fn validate_call(
        &self,
        span: Span,
        args: &TyCallArgs,
        oracle: TypingOracleCtx,
    ) -> Result<Ty, TypingOrInternalError> {
        oracle.validate_fn_call(span, &LIST.callable, args)?;

        if let Some(arg) = args.pos.first() {
            // This is infallible after the check above.
            let item = oracle.iter_item(Spanned {
                span,
                node: &arg.node,
            })?;
            return Ok(Ty::list(item));
        }

        Ok(Ty::any_list())
    }
}

pub(crate) fn register_list(globals: &mut GlobalsBuilder) {
    globals.type_symbol::<StarlarkList>();
}
