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

use crate::values::FrozenValueTyped;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::dict::StarlarkDict;
use crate::values::int::IntView;
use crate::values::int::inline_int::InlineInt;
use crate::values::list::StarlarkList;
use crate::values::none::NoneType;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::typing::StarlarkNever;

/// A Starlark value that exists without a heap: what the default of a parameter of a
/// [`#[starlark_module]`](macro@crate::starlark_module) function is written as.
/// `#[starlark(default = ..)]` takes an expression of any type that implements this.
///
/// An argument that is left out is bound from the default by the parameter's own conversion,
/// exactly as if the default had been passed. So a default is written as the Starlark value
/// and not as what the conversion makes of it: `NoneType` for a `NoneOr<T>` parameter, `true`
/// for a `NoneOr<bool>` one.
///
/// A default is made once, is shared by every call and is immutable, like the default of a
/// `def` whose module is frozen. A function that wants a fresh list when none is passed takes
/// `None` and allocates in its body.
///
/// The implementers are `bool`, `i32`, [`NoneType`], [`EmptyList`], [`EmptyDict`] and the
/// handles at the `'static` brand, which cover a
/// [`const_frozen_string!`](crate::const_frozen_string) and a
/// [`static_starlark_value!`](crate::static_starlark_value) through its `unpack`. A `&str`
/// is not a Starlark value, so it is not one; for a string literal the macro makes the static
/// string, so that `default = "x"` can be written.
///
/// An `int` exists without a heap if it is stored inline, which not every `i32` is where a
/// pointer has 32 bits. So `i32` is an implementer only where a pointer has 64, and an integer
/// literal, as in `default = 3`, is a default on every target: the macro has it checked when
/// the function is compiled.
///
/// # A default is of its parameter's type
///
/// The type the parameter converts from has to [take](crate::values::type_expr::Accepts) the
/// default's [`Type`](Self::Type): it is that type, or the parameter's type is a union and the
/// default is of one of the alternatives, or the default is the empty list or dict and the
/// parameter takes a list or a dict. A function with any other default does not compile:
///
/// ```compile_fail
/// # use starlark::environment::GlobalsBuilder;
/// # use starlark::starlark_module;
/// # use starlark::values::none::NoneType;
/// #[starlark_module]
/// fn globals(builder: &mut GlobalsBuilder) {
///     fn f(#[starlark(default = NoneType)] n: i32) -> anyhow::Result<i32> {
///         Ok(n)
///     }
/// }
/// ```
///
/// A parameter of type [`Value`] takes every value, so any default is of its type. What is
/// not checked is a value of the right type that the conversion refuses, such as `-1` for a
/// `u32`: that fails when a call leaves the argument out.
pub trait ParamDefault {
    /// The Starlark type of the value.
    type Type<'v>: StarlarkTypeExpr<'v>;

    /// The value.
    fn param_default(self) -> Value<'static>;
}

impl ParamDefault for bool {
    type Type<'v> = bool;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        Value::new_bool(self)
    }
}

/// An `int` that is stored inline, which every `i32` is where a pointer has 64 bits. Where it
/// has 32 an inline int has 29 bits, so an `i32` is not a default there, and an `int` default
/// is written as a literal.
#[cfg(target_pointer_width = "64")]
impl ParamDefault for i32 {
    type Type<'v> = IntView<'v>;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        Value::new_int(InlineInt::from_i32(self))
    }
}

impl ParamDefault for NoneType {
    type Type<'v> = NoneType;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        Value::new_none()
    }
}

/// A value that says nothing about its type, so it is a default of a [`Value`] parameter only.
impl ParamDefault for Value<'static> {
    type Type<'v> = Value<'v>;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        self
    }
}

/// The type expression has to be one at every brand, so that the default's type can be named
/// at the parameter's: a type with a lifetime parameter does not qualify.
impl<T: for<'a> StarlarkTypeExpr<'a>> ParamDefault for ValueOf<'static, T> {
    type Type<'v> = T;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        self.to_value()
    }
}

impl<T: for<'a> StarlarkValue<'a>> ParamDefault for ValueTyped<'static, T> {
    type Type<'v> = <T as StarlarkValue<'v>>::Type;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        self.to_value()
    }
}

impl<T: for<'a> StarlarkValue<'a>> ParamDefault for FrozenValueTyped<'static, T> {
    type Type<'v> = <T as StarlarkValue<'v>>::Type;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        self.to_value()
    }
}

/// The immutable empty list, as a default. It is a `list[typing.Never]`, which every list
/// type and every iterable takes.
#[derive(Debug, Clone, Copy, dupe::Dupe)]
pub struct EmptyList;

impl ParamDefault for EmptyList {
    type Type<'v> = StarlarkList<'v, StarlarkNever>;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        Value::new_empty_list()
    }
}

/// The immutable empty dict, as a default. It is a `dict[typing.Never, typing.Never]`, which
/// every dict type takes.
#[derive(Debug, Clone, Copy, dupe::Dupe)]
pub struct EmptyDict;

impl ParamDefault for EmptyDict {
    type Type<'v> = StarlarkDict<'v, StarlarkNever, StarlarkNever>;

    #[inline(always)]
    fn param_default(self) -> Value<'static> {
        Value::new_empty_dict()
    }
}

/// The default of a parameter that takes no value when its argument is left out: the
/// parameter is an `Option<T>`, and an argument that is left out is `None`.
///
/// ```ignore
/// fn getattr<'v>(
///     a: Value<'v>,
///     attr: &str,
///     #[starlark(default = Absent)] default: Option<Value<'v>>,
/// ) -> ..
/// ```
///
/// It is not a Starlark value, so no program can pass it, and the documentation shows such a
/// default as `...`, with the parameter's type as `T`.
///
/// # When it is the right choice
///
/// An optional parameter is, in order of preference:
///
/// 1. A parameter with a value as its default, where leaving the argument out is the same as
///    passing that value: `#[starlark(default = 0)] x: i32`.
/// 2. `#[starlark(default = NoneType)] x: NoneOr<T>`, where it is not the same as passing any
///    value of `T`. A caller can then pass the `None` on.
/// 3. `Absent`, where `None` must not be accepted in place of the argument. That is a
///    parameter for which `None` is itself a value (`getattr`'s `default`), and a parameter
///    that the Starlark specification makes optional without saying that it may be `None`
///    (`list.pop`'s index), together with what the standard library has next to those.
#[derive(Debug, Clone, Copy, dupe::Dupe)]
pub struct Absent;
