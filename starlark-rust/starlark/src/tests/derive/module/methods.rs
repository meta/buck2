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

use allocative::Allocative;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::assert::Assert;
use crate::environment::MethodsBuilder;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::values::StarlarkValue;
use crate::values::Value;

#[derive(
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{:?}", self)]
struct Applaud {
    value: i32,
}

#[starlark_module]
fn methods(builder: &mut MethodsBuilder) {
    fn test_method(#[starlark(this)] receiver: Value, this: i32) -> anyhow::Result<i32> {
        let applauld = receiver.downcast_ref::<Applaud>().unwrap();
        Ok(applauld.value + this)
    }

    #[starlark(attribute)]
    fn callable<'v>(this: Value<'v>) -> anyhow::Result<Value<'v>> {
        Ok(this)
    }

    #[starlark(attribute)]
    fn value(this: Value) -> anyhow::Result<i32> {
        Ok(this.downcast_ref::<Applaud>().unwrap().value)
    }
}

#[starlark_value(type = "applaud", methods = methods)]
impl<'v> StarlarkValue<'v> for Applaud {
    fn invoke(
        _this: &Self,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        Ok(eval.heap().alloc(args.len()?).to_value())
    }
}

#[test]
fn test_receiver_can_be_named_anything() {
    let mut a = Assert::new();
    a.globals_add(|g| g.set("x", Applaud { value: 10 }));
    a.eq("13", "x.test_method(this=3)");
    a.eq("10", "x.value");
    a.eq("2", "x.callable(1, named=2)");
    a.fail("x.value(1)", "Operation `call()` not supported");
}

#[derive(
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{:?}", self)]
struct TwoModules;

#[starlark_module]
fn first_methods(builder: &mut MethodsBuilder) {
    #[starlark(attribute)]
    fn one(this: Value) -> anyhow::Result<i32> {
        let _ = this;
        Ok(1)
    }

    fn a_method_with_a_name_of_more_than_sixteen_bytes(this: Value) -> anyhow::Result<i32> {
        let _ = this;
        Ok(16)
    }
}

#[starlark_module]
fn second_methods(builder: &mut MethodsBuilder) {
    fn two(this: Value, #[starlark(default = 2)] x: i32) -> anyhow::Result<i32> {
        let _ = this;
        Ok(x)
    }
}

#[starlark_value(type = "two_modules", methods = [first_methods, second_methods])]
impl<'v> StarlarkValue<'v> for TwoModules {}

#[test]
fn test_a_type_has_the_members_of_two_modules() {
    let mut a = Assert::new();
    a.globals_add(|g| g.set("x", TwoModules));
    a.eq("1", "x.one");
    a.eq("2", "x.two()");
    a.eq("5", "x.two(x = 5)");
    a.eq("16", "x.a_method_with_a_name_of_more_than_sixteen_bytes()");
    a.eq("2", "getattr(x, 'two')()");
    a.eq(
        "['a_method_with_a_name_of_more_than_sixteen_bytes', 'one', 'two']",
        "dir(x)",
    );
    a.is_true("hasattr(x, 'one') and not hasattr(x, 'three')");
    // A name that is not a member, of a length that one is.
    a.fail("x.six", "no attribute `six`");
    a.fail(
        "x.a_method_with_a_name_of_more_than_sixteen_bytez()",
        "no attribute",
    );
}
