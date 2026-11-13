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

pub(crate) mod param;

use std::convert::Infallible;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::sync::atomic::AtomicPtr;

use allocative::Allocative;
use dupe::Dupe;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::docs::DocItem;
use crate::docs::DocString;
use crate::docs::DocType;
use crate::private::Private;
use crate::static_starlark_value;
use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::TyBasic;
use crate::typing::callable::TyCallable;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::list::ListRef;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::typing::TypeCompiled;
use crate::values::typing::callable::param::StarlarkCallableParamAny;
use crate::values::typing::callable::param::StarlarkCallableParamSpec;

#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("{}", Self::TYPE)]
pub(crate) struct TypingCallable;

#[starlark_value(type = "typing.Callable")]
impl<'v> StarlarkValue<'v> for TypingCallable {
    fn documentation(&self) -> DocItem {
        DocItem::Type(DocType {
            docs: DocString::from_docstring(
                crate::docs::DocStringKind::Rust,
                "\
Type annotation for callable objects (e.g. functions).

This is a generic type whose parameters indicate the argument types and return type of the object.

# Examples

```starlark
def f(i: int, s: str) -> bool:
...

# would be matched by:

typing.Callable[[int, str], bool]
```

See also [`typing.Callable` in the Python documentation][1].

[1]: https://docs.python.org/3/library/typing.html#typing.Callable",
            ),
            ..DocType::from_starlark_value::<Self>()
        })
    }

    fn eval_type(&self) -> Option<Ty> {
        Some(StarlarkCallable::<StarlarkCallableParamAny, Value<'static>>::starlark_type_repr())
    }

    fn at2(
        &self,
        param_types: Value<'v>,
        ret: Value<'v>,
        heap: Heap<'v>,
        _private: Private,
    ) -> crate::Result<Value<'v>> {
        let param_types = <&ListRef>::unpack_value_err(param_types)?;
        let ret = TypeCompiled::new(ret, heap)?.as_ty().dupe();
        let param_types: Vec<Ty> = param_types
            .iter()
            .map(|p| Ok(TypeCompiled::new(p, heap)?.as_ty().dupe()))
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(heap.alloc_simple(TypingCallableAt2 {
            callable: TyCallable::new(ParamSpec::pos_only(param_types, []), ret),
        }))
    }
}

static_starlark_value!(CALLABLE: TypingCallable = TypingCallable);

impl<'fv> AllocFrozenValue<'fv> for TypingCallable {
    type Type = Self;

    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        CALLABLE.at().into()
    }
}

#[derive(
    Allocative,
    Debug,
    ProvidesStaticType,
    NoSerialize,
    derive_more::Display,
    StarlarkPagable
)]
#[display("{}", callable)]
pub(crate) struct TypingCallableAt2 {
    #[starlark_pagable(pagable)]
    callable: TyCallable,
}

#[starlark_value(type = "typing.Callable")]
impl<'v> StarlarkValue<'v> for TypingCallableAt2 {
    fn eval_type(&self) -> Option<Ty> {
        Some(Ty::basic(TyBasic::Callable(self.callable.dupe())))
    }
}

/// Marker for a callable value. Can be used in function signatures
/// for better documentation and type checking.
#[derive(Allocative, StarlarkPagable)]
#[allocative(bound = "")]
pub struct StarlarkCallable<
    'v,
    P: StarlarkCallableParamSpec = StarlarkCallableParamAny,
    R: StarlarkTypeExpr<'static> = Value<'static>,
>(pub Value<'v>, PhantomData<PhantomData<AtomicPtr<(P, R)>>>);

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Copy
    for StarlarkCallable<'v, P, R>
{
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Clone
    for StarlarkCallable<'v, P, R>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Dupe
    for StarlarkCallable<'v, P, R>
{
}

// SAFETY: the static type is the same type with the heap lifetime erased; `P` and `R` only
// describe the signature and carry no heap lifetime.
unsafe impl<'v, P: StarlarkCallableParamSpec + 'static, R: StarlarkTypeExpr<'static> + 'static>
    ProvidesStaticType<'v> for StarlarkCallable<'v, P, R>
{
    type StaticType = StarlarkCallable<'static, P, R>;
}

impl<P: StarlarkCallableParamSpec + 'static, R: StarlarkTypeExpr<'static> + 'static> IsStaticType
    for StarlarkCallable<'static, P, R>
{
    type Reinfect<'lt> = StarlarkCallable<'lt, P, R>;
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Debug
    for StarlarkCallable<'v, P, R>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StarlarkCallable").field(&self.0).finish()
    }
}

// TODO(nga): implement `#[trace(bound = "")]`.
unsafe impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Trace<'v>
    for StarlarkCallable<'v, P, R>
{
    fn trace(&mut self, tracer: &Tracer<'v>) {
        self.0.trace(tracer);
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> StarlarkCallable<'v, P, R> {
    /// Wrap the value.
    pub fn unchecked_new(value: Value<'v>) -> Self {
        StarlarkCallable(value, PhantomData)
    }

    /// Erase parameter and return types.
    pub fn erase(self) -> StarlarkCallable<'v> {
        StarlarkCallable::unchecked_new(self.0)
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> StarlarkTypeExpr<'v>
    for StarlarkCallable<'v, P, R>
{
    type Get = Self;

    fn starlark_type_repr() -> Ty {
        // TODO(nga): implement the same machinery for `typing.Callable`.
        Ty::callable(P::params(), R::starlark_type_repr())
    }

    fn get(value: Value<'v>) -> Option<Self> {
        value
            .vtable()
            .starlark_value
            .ty
            .HAS_invoke
            .then(|| StarlarkCallable::unchecked_new(value))
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> StarlarkTypeExprStable<'v>
    for StarlarkCallable<'v, P, R>
{
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> UnpackValue<'v>
    for StarlarkCallable<'v, P, R>
{
    type Type = Self;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, Self>, get: Self) -> Result<Self, Infallible> {
        Ok(get)
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> AllocValue<'v>
    for StarlarkCallable<'v, P, R>
{
    type Type = Self;

    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::new_unchecked(self.0)
    }
}

fn _assert_sync_send() {
    fn _assert<T: Sync + Send>() {}
    // Type-repr parameters inside a `Freeze` carrier are written at `'static`, since the
    // carrier's `Frozen<'fv>` passes them through unchanged; frozen values are `Send + Sync`,
    // so the marker has to be both at that brand.
    _assert::<StarlarkCallable<'static, (Value,), Value>>();
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Freeze<'v>
    for StarlarkCallable<'v, P, R>
{
    type Frozen<'fv> = StarlarkCallable<'fv, P, R>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(StarlarkCallable::unchecked_new(Freeze::freeze(
            self.0, freezer,
        )?))
    }
}

/// More strict version of [`StarlarkCallable`].
///
/// This checks not only that the value is callable,
/// but also that it is a callable with the correct signature.
///
/// The implementation uses starlark-rust typechecker with all its limitations.
/// For example, if there are optional parameters in both value-def and this signature,
/// signature matching is ignored at the time of writing.
///
/// Unpacking with this type is expensive:
/// usually it is OK to use it for code executed once at top-level scope (like `rule()`),
/// but not for code executed many times (like `partial()`).
#[derive(Allocative)]
#[allocative(bound = "")]
pub struct StarlarkCallableChecked<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>>(
    pub Value<'v>,
    PhantomData<AtomicPtr<(P, R)>>,
);

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Clone
    for StarlarkCallableChecked<'v, P, R>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Copy
    for StarlarkCallableChecked<'v, P, R>
{
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Dupe
    for StarlarkCallableChecked<'v, P, R>
{
}

// SAFETY: as for `StarlarkCallable`.
unsafe impl<'v, P: StarlarkCallableParamSpec + 'static, R: StarlarkTypeExpr<'static> + 'static>
    ProvidesStaticType<'v> for StarlarkCallableChecked<'v, P, R>
{
    type StaticType = StarlarkCallableChecked<'static, P, R>;
}

impl<P: StarlarkCallableParamSpec + 'static, R: StarlarkTypeExpr<'static> + 'static> IsStaticType
    for StarlarkCallableChecked<'static, P, R>
{
    type Reinfect<'lt> = StarlarkCallableChecked<'lt, P, R>;
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Debug
    for StarlarkCallableChecked<'v, P, R>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StarlarkCallableChecked")
            .field(&self.0)
            .finish()
    }
}

unsafe impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> Trace<'v>
    for StarlarkCallableChecked<'v, P, R>
{
    fn trace(&mut self, tracer: &Tracer<'v>) {
        let StarlarkCallableChecked(value, phantom) = self;
        value.trace(tracer);
        phantom.trace(tracer);
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> AllocValue<'v>
    for StarlarkCallableChecked<'v, P, R>
{
    type Type = Self;

    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::new_unchecked(self.0)
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>>
    StarlarkCallableChecked<'v, P, R>
{
    /// Convert to [`StarlarkCallable`].
    pub fn to_unchecked(self) -> StarlarkCallable<'v, P, R> {
        StarlarkCallable::unchecked_new(self.0)
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> StarlarkTypeExpr<'v>
    for StarlarkCallableChecked<'v, P, R>
{
    type Get = Self;

    fn starlark_type_repr() -> Ty {
        StarlarkCallable::<'v, P, R>::starlark_type_repr()
    }

    fn get(value: Value<'v>) -> Option<Self> {
        Self::unpack_value(value).ok().flatten()
    }
}

impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> StarlarkTypeExprStable<'v>
    for StarlarkCallableChecked<'v, P, R>
{
}

#[derive(thiserror::Error, Debug)]
#[error("Callable of type `{actual}` does not match the expected signature `{expected}`")]
struct CallableSignatureMismatch {
    expected: Ty,
    actual: Ty,
}

/// The runtime type `typing.Callable` accepts any callable, so a signature mismatch is a
/// conversion failure, not a type mismatch.
impl<'v, P: StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>> UnpackValue<'v>
    for StarlarkCallableChecked<'v, P, R>
{
    type Type = StarlarkCallable<'v, P, R>;
    type Error = crate::Error;

    fn unpack(
        value: ValueOf<'v, Self::Type>,
        _get: StarlarkCallable<'v, P, R>,
    ) -> crate::Result<Self> {
        let value = value.to_value();
        // We need generic statics to cache this.
        let ty = Ty::callable(P::params(), R::starlark_type_repr());

        let actual = Ty::of_value(value);
        match actual.check_intersects(&ty)? {
            true => Ok(StarlarkCallableChecked(value, PhantomData)),
            false => Err(crate::Error::new_value(CallableSignatureMismatch {
                expected: ty,
                actual,
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use starlark_derive::starlark_module;

    use crate as starlark;
    use crate::assert;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::values::int::IntView;
    use crate::values::none::NoneType;
    use crate::values::string::StarlarkStr;
    use crate::values::typing::StarlarkCallable;
    use crate::values::typing::StarlarkCallableChecked;

    #[test]
    fn test_callable_runtime() {
        assert::is_true("isinstance(lambda: None, typing.Callable)");
        assert::is_true("isinstance(len, typing.Callable)");
        assert::is_true("Rec = record(); isinstance(Rec, typing.Callable)");
        assert::is_false("isinstance(37, typing.Callable)");
    }

    #[test]
    fn test_callable_pass_compile_time() {
        assert::pass(
            r#"
Rec = record()

def foo(x: typing.Callable):
    pass

def bar():
    foo(len)
    foo(lambda x: 1)
    foo(Rec)
"#,
        );
    }

    #[test]
    fn test_callable_fail_compile_time() {
        assert::fail(
            r#"
def foo(x: typing.Callable):
    pass

def bar():
    foo(1)
"#,
            "Expected type",
        );
    }

    #[starlark_module]
    fn my_module(globals: &mut GlobalsBuilder) {
        fn accept_f(
            #[starlark(require=pos)] _x: StarlarkCallable<(StarlarkStr,), IntView<'static>>,
        ) -> anyhow::Result<NoneType> {
            Ok(NoneType)
        }
    }

    #[test]
    fn test_native_callable_pass() {
        let mut a = Assert::new();
        a.globals_add(my_module);
        a.pass(
            r#"
def f(x: str) -> int:
    return len(x)

def test():
    accept_f(f)
"#,
        );
    }

    #[test]
    fn test_native_callable_fail_compile_time_wrong_param_type() {
        let mut a = Assert::new();
        a.globals_add(my_module);
        a.fail(
            r#"
def f(x: list) -> int:
    return 1

def test():
    accept_f(f)
"#,
            "Expected type `typing.Callable[[str], int]` but got",
        );
    }

    #[test]
    fn test_native_callable_fail_compile_time_wrong_param_count() {
        let mut a = Assert::new();
        a.globals_add(my_module);
        a.fail(
            r#"
def f() -> int:
    return 1

def test():
    accept_f(f)
"#,
            "Expected type `typing.Callable[[str], int]` but got",
        );
    }

    #[test]
    fn test_typing_callable_pass() {
        let a = Assert::new();
        a.pass(
            r#"
def accept_f(x: typing.Callable[[str], int]) -> None:
    pass

def f(x: str) -> int:
    return len(x)

def test():
    accept_f(f)
"#,
        );
    }

    #[test]
    fn test_typing_callable_fail_compile_time_wrong_param_type() {
        let a = Assert::new();
        a.fail(
            r#"
def accept_f(x: typing.Callable[[str], int]) -> None:
    pass

def f(x: list) -> int:
    return 1

def test():
    accept_f(f)
"#,
            "Expected type `typing.Callable[[str], int]` but got",
        );
    }

    #[test]
    fn test_typing_callable_fail_compile_time_wrong_param_count() {
        let a = Assert::new();
        a.fail(
            r#"
def accept_f(x: typing.Callable[[str], int]) -> None:
    pass

def f() -> int:
    return 1

def test():
    accept_f(f)
"#,
            "Expected type `typing.Callable[[str], int]` but got",
        );
    }

    #[test]
    fn test_callable_checked_runtime() {
        #[starlark_module]
        fn module(globals: &mut GlobalsBuilder) {
            fn accept_f(
                #[starlark(require=pos)] _f: StarlarkCallableChecked<(), NoneType>,
            ) -> anyhow::Result<NoneType> {
                Ok(NoneType)
            }

            fn good() -> anyhow::Result<NoneType> {
                Ok(NoneType)
            }

            fn bad() -> anyhow::Result<i32> {
                Ok(10)
            }
        }

        let mut a = Assert::new();
        a.globals_add(module);

        a.pass("accept_f(good)");

        a.fail(
            r#"
def test():
    x = noop(bad) # Hide the type from static typechecker.
    accept_f(x)

test()
        "#,
            "does not match the expected signature `typing.Callable[[], None]`",
        );
    }
}
