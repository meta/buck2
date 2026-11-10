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
use crate::environment::Methods;
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

starlark::methods_static!(APPLAUD_METHODS = methods);

#[starlark_value(type = "applaud")]
impl<'v> StarlarkValue<'v> for Applaud {
    fn get_methods() -> Option<&'static Methods> {
        Some(APPLAUD_METHODS.methods())
    }

    fn invoke(
        &self,
        _me: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        Ok(eval.heap().alloc(args.len()?))
    }
}

#[test]
fn test_receiver_can_be_named_anything() {
    let mut a = Assert::new();
    a.globals_add(|g| g.set_representation("x", Applaud { value: 10 }));
    a.eq("13", "x.test_method(this=3)");
    a.eq("10", "x.value");
    a.eq("2", "x.callable(1, named=2)");
    a.fail("x.value(1)", "Operation `call()` not supported");
}
