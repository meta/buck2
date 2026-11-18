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

use crate::typing::ParamSpec;
use crate::values::type_expr::StarlarkTypeExpr;

/// Type parameter for [`StarlarkCallable`](crate::values::typing::callable::StarlarkCallable)
/// describing the expected parameters of the callable.
pub trait StarlarkCallableParamSpec {
    /// Get the parameter specification for the callable.
    fn params() -> ParamSpec;
}

/// Indicates that a callable accepts any number of positional and keyword arguments.
pub struct StarlarkCallableParamAny;

/// `*args` and `**kwargs` parameters.
impl StarlarkCallableParamSpec for StarlarkCallableParamAny {
    fn params() -> ParamSpec {
        ParamSpec::any()
    }
}

/// No parameters.
impl StarlarkCallableParamSpec for () {
    fn params() -> ParamSpec {
        ParamSpec::pos_only([], [])
    }
}

/// Single positional-only parameter.
impl<A: StarlarkTypeExpr<'static>> StarlarkCallableParamSpec for (A,) {
    fn params() -> ParamSpec {
        ParamSpec::pos_only([A::starlark_type_repr()], [])
    }
}

/// Two positional-only parameters.
impl<A: StarlarkTypeExpr<'static>, B: StarlarkTypeExpr<'static>> StarlarkCallableParamSpec
    for (A, B)
{
    fn params() -> ParamSpec {
        ParamSpec::pos_only([A::starlark_type_repr(), B::starlark_type_repr()], [])
    }
}

/// Three positional-only parameters.
impl<A: StarlarkTypeExpr<'static>, B: StarlarkTypeExpr<'static>, C: StarlarkTypeExpr<'static>>
    StarlarkCallableParamSpec for (A, B, C)
{
    fn params() -> ParamSpec {
        ParamSpec::pos_only(
            [
                A::starlark_type_repr(),
                B::starlark_type_repr(),
                C::starlark_type_repr(),
            ],
            [],
        )
    }
}

/// Four positional-only parameters.
impl<
    A: StarlarkTypeExpr<'static>,
    B: StarlarkTypeExpr<'static>,
    C: StarlarkTypeExpr<'static>,
    D: StarlarkTypeExpr<'static>,
> StarlarkCallableParamSpec for (A, B, C, D)
{
    fn params() -> ParamSpec {
        ParamSpec::pos_only(
            [
                A::starlark_type_repr(),
                B::starlark_type_repr(),
                C::starlark_type_repr(),
                D::starlark_type_repr(),
            ],
            [],
        )
    }
}
