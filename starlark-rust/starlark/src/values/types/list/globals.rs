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
use starlark_derive::starlark_module;

use crate as starlark;
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
use crate::values::Heap;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::function::SpecialBuiltinFunction;
use crate::values::list::AllocList;
use crate::values::list::ListRef;
use crate::values::list::ListType;
use crate::values::list::value::FrozenList;
use crate::values::typing::StarlarkIter;

#[derive(
    Allocative, Hash, Eq, PartialEq, Ord, PartialOrd, Clone, Debug, Pagable
)]
struct ListFunctionTy;

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

#[starlark_module]
pub(crate) fn register_list(globals: &mut GlobalsBuilder) {
    /// [list](
    /// https://github.com/bazelbuild/starlark/blob/master/spec.md#list
    /// ): construct a list.
    ///
    /// `list(x)` returns a new list containing the elements of the
    /// iterable sequence x.
    ///
    /// With no argument, `list()` returns a new empty list.
    ///
    /// ```
    /// # starlark::assert::all_true(r#"
    /// list()        == []
    /// list((1,2,3)) == [1, 2, 3]
    /// # "#);
    /// # starlark::assert::fail(r#"
    /// list("strings are not iterable") # error: expected `typing.Iterable`
    /// # "#, r#"expected `typing.Iterable`"#);
    /// ```
    #[starlark(
    as_type = FrozenList,
    speculative_exec_safe,
    special_builtin_function = SpecialBuiltinFunction::List,
    ty_custom_function = ListFunctionTy,
    )]
    fn list<'v>(
        #[starlark(require = pos)] a: Option<ValueOf<'v, StarlarkIter<Value<'v>>>>,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, ListType<Value<'v>>>> {
        Ok(ValueOf::new_unchecked(if let Some(a) = a {
            if let Some(xs) = ListRef::from_value(a.to_value()) {
                heap.alloc_list(xs.content())
            } else {
                let it = a.to_value().iterate(heap)?;
                heap.alloc(AllocList(it))
            }
        } else {
            heap.alloc(AllocList::EMPTY)
        }))
    }
}
