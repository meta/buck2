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

use std::marker::PhantomData;

use starlark_derive::starlark_module;

use crate as starlark;
use crate::assert::Assert;
use crate::environment::GlobalsBuilder;
use crate::environment::MethodsBuilder;
use crate::typing::Ty;
use crate::values::AllocValue;
use crate::values::Heap;
use crate::values::Value;
use crate::values::none::NoneType;
use crate::values::type_repr::StarlarkTypeRepr;

#[starlark_module]
fn global_builder<T: Default, U>(globals: &mut GlobalsBuilder)
where
    U: std::fmt::Display + Default,
{
    const MY_STR: &str = &U::default().to_string();
}

#[allow(dead_code)]
struct CustomNone<T>(PhantomData<T>);

impl<T> StarlarkTypeRepr for CustomNone<T> {
    type Canonical = NoneType;

    /// The representation of a type that a user would use verbatim in starlark type annotations
    fn starlark_type_repr() -> Ty {
        NoneType::starlark_type_repr()
    }
}

impl<'v, T> AllocValue<'v> for CustomNone<T> {
    fn alloc_value(self, _heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        Value::new_none()
    }
}

#[starlark_module]
fn method_builder<T: Default, U>(globals: &mut MethodsBuilder)
where
    U: std::fmt::Display + Default,
{
    // Just check that this compiles
    #[starlark(attribute)]
    fn test_attribute(this: u32) -> starlark::Result<CustomNone<T>> {
        let _u = U::default().to_string();
        let _t = T::default();
        Ok(CustomNone(PhantomData))
    }
}

#[test]
fn test_generic_builder() {
    let mut a = Assert::new();
    a.globals_add(global_builder::<u8, u8>);
    a.eq("\"0\"", "MY_STR");
}
