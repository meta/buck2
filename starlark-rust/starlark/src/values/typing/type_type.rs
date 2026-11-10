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

use std::convert::Infallible;

use crate::typing::Ty;
use crate::typing::TyStarlarkValue;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::typing::ty::AbstractType;

/// Represent a type of type. (For example, an expression `int` is valid for this type.)
pub struct TypeType(());

impl StarlarkTypeRepr for TypeType {
    type Canonical = AbstractType;

    fn starlark_type_repr() -> Ty {
        <Self::Canonical as StarlarkTypeRepr>::starlark_type_repr()
    }
}

impl<'v> StarlarkTypeExpr<'v> for TypeType {
    type Get = TypeType;

    fn get(value: Value<'v>) -> Option<Self::Get> {
        TyStarlarkValue::is_type_from_vtable(&value.vtable().starlark_value.ty)
            .then_some(TypeType(()))
    }
}

impl<'v> StarlarkTypeExprStable<'v> for TypeType {}

impl<'v> UnpackValue<'v> for TypeType {
    type Type = Self;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, Self>, get: Self) -> Result<Self, Infallible> {
        Ok(get)
    }
}

#[cfg(test)]
mod tests {
    use starlark_derive::starlark_module;

    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::values::none::NoneType;
    use crate::values::typing::type_type::TypeType;

    #[test]
    fn test() {
        #[starlark_module]
        fn module(globals: &mut GlobalsBuilder) {
            fn takes_type(#[starlark(require = pos)] _t: TypeType) -> anyhow::Result<NoneType> {
                Ok(NoneType)
            }
        }

        let mut a = Assert::new();
        a.globals_add(module);
        a.pass("takes_type(int)");
        a.pass("takes_type(list[str] | None)");
        a.fail(
            "takes_type(1)",
            "Type of parameter `_t` doesn't match, expected `type`,",
        );
    }
}
