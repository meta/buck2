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

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::ValueOf;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::num::value::Num;
use crate::values::types::num::value::NumRef;

/// Unpack `int` or `float` into `f64`.
#[derive(ProvidesStaticType)]
pub struct UnpackFloat(pub f64);

impl StarlarkTypeRepr for UnpackFloat {
    type Canonical = <Num as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        <Self::Canonical as StarlarkTypeRepr>::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for UnpackFloat {
    type Type = <NumRef<'v> as UnpackValue<'v>>::Type;
    type Error = <NumRef<'v> as UnpackValue<'v>>::Error;

    fn unpack(
        value: ValueOf<'v, Self::Type>,
        get: <Self::Type as StarlarkTypeExpr<'v>>::Get,
    ) -> Result<Self, Self::Error> {
        Ok(UnpackFloat(NumRef::unpack(value, get)?.as_float()))
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::UnpackValue;
    use crate::values::Value;
    use crate::values::float::UnpackFloat;

    #[test]
    fn test_unpack_float() {
        Heap::temp(|heap| {
            assert_eq!(
                1.0,
                UnpackFloat::unpack_value(Value::testing_new_int(1))
                    .unwrap()
                    .unwrap()
                    .0
            );
            assert_eq!(
                1.0,
                UnpackFloat::unpack_value(heap.alloc(1.0))
                    .unwrap()
                    .unwrap()
                    .0
            );
        });
    }
}
