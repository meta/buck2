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

use std::convert::Infallible;
use std::marker::PhantomData;

use allocative::Allocative;
use starlark_derive::NoSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::docs::DocItem;
use crate::docs::DocString;
use crate::docs::DocType;
use crate::static_starlark_value;
use crate::typing::Ty;
use crate::typing::TyStarlarkValue;
use crate::values::AllocFrozenValue;
use crate::values::FrozenHeap;
use crate::values::StarlarkValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::type_repr::StarlarkTypeRepr;

/// `typing.Iterable[T]`: a type marker, and the check that a value is iterable.
///
/// The check is the one the type's matcher does: that the value iterates. What it yields is
/// not looked at, so `T` is what the caller asserts of the items, not something verified.
pub struct StarlarkIter<T: StarlarkTypeRepr>(PhantomData<T>);

impl<'v, T: StarlarkTypeRepr> UnpackValue<'v> for StarlarkIter<T> {
    type Error = Infallible;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        Ok(
            TyStarlarkValue::is_iterable(&value.vtable().starlark_value.ty)
                .then_some(StarlarkIter(PhantomData)),
        )
    }
}

impl<T: StarlarkTypeRepr> StarlarkTypeRepr for StarlarkIter<T> {
    type Canonical = StarlarkIter<T::Canonical>;

    fn starlark_type_repr() -> Ty {
        Ty::iter(T::starlark_type_repr())
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for StarlarkIter<T> {
    /// The handle, which is what iterates.
    type Get = ValueOf<'v, StarlarkIter<T>>;

    fn get(value: Value<'v>) -> Option<Self::Get> {
        TyStarlarkValue::is_iterable(&value.vtable().starlark_value.ty)
            .then(|| ValueOf::new_unchecked(value))
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExprStable<'v> for StarlarkIter<T> {}

#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("{}", Self::TYPE)]
pub(crate) struct TypingIterable;

#[starlark_value(type = "typing.Iterable")]
impl<'v> StarlarkValue<'v> for TypingIterable {
    fn documentation(&self) -> DocItem {
        DocItem::Type(DocType {
            docs: DocString::from_docstring(
                crate::docs::DocStringKind::Rust,
                "\
Represents a type that can be [iterated][1].

`Iterable` takes one type argument, which may indicate the type of values yielded by the `Iterable`
instance. Specifying the type to be iterated over is not currently supported.

See also [`typing.Iterable` in the Python documentation][2].

[1]: https://github.com/bazelbuild/starlark/blob/master/spec.md#iteration
[2]: https://docs.python.org/3/library/typing.html#typing.Iterable",
            ),
            ..DocType::from_starlark_value::<Self>()
        })
    }

    fn eval_type(&self) -> Option<Ty> {
        Some(Ty::iter(Ty::any()))
    }

    // TODO(nga): support `[]`.
}

static_starlark_value!(ITERABLE: TypingIterable = TypingIterable);

impl<'fv> AllocFrozenValue<'fv> for TypingIterable {
    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> Value<'fv> {
        ITERABLE.at().to_value()
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_iterable_runtime() {
        assert::is_true("isinstance([1, 2, 3], typing.Iterable)");
        assert::is_true("isinstance((1, 2, 3), typing.Iterable)");
        assert::is_true("isinstance(range(10), typing.Iterable)");
        assert::is_false("isinstance('', typing.Iterable)");
        assert::is_false("isinstance(1, typing.Iterable)");
    }

    #[test]
    fn test_iterable_compile_time_pass() {
        assert::pass(
            r#"
def foo(x: typing.Iterable):
    pass

def bar():
    foo([1, 2, 3])
"#,
        );
    }

    #[test]
    fn test_iterable_compile_time_fail() {
        assert::fail(
            r#"
def foo(x: typing.Iterable):
    pass

def bar():
    foo(1)
"#,
            "Expected type `typing.Iterable`",
        );
    }
}
