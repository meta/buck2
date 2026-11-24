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
use derive_more::Display;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::assert::Assert;
use crate::starlark_simple_value;
use crate::values::StarlarkValue;

macro_rules! export_hello_attribute {
    () => {
        fn get_attr(
            this: starlark::values::ValueTyped<'v, Self>,
            attr: &str,
            heap: starlark::values::Heap<'v>,
        ) -> Option<starlark::values::Value<'v>> {
            match attr {
                "hello" => Some(heap.alloc(&this.hello).to_value()),
                _ => None,
            }
        }

        fn dir_attr(_this: starlark::values::ValueTyped<'v, Self>) -> Vec<String> {
            vec!["hello".to_owned()]
        }
    };
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{:?}", self)]
struct Example {
    hello: String,
}
starlark_simple_value!(Example);

#[starlark_value(type = "example")]
impl<'v> StarlarkValue<'v> for Example {
    export_hello_attribute!();
}

#[test]
fn test_attribute_from_item_macro() {
    let mut a = Assert::new();
    a.globals_add(|builder| {
        builder.set(
            "example",
            Example {
                hello: "world".to_owned(),
            },
        )
    });

    a.eq("example.hello", "\"world\"");
    a.eq("dir(example)", "[\"hello\"]");
    a.eq("def f():\n    return example.hello\nf()", "\"world\"");
}

macro_rules! shout_repr {
    () => {
        fn collect_repr(this: starlark::values::ValueTyped<'v, Self>, collector: &mut String) {
            collector.push_str(&this.hello.to_uppercase());
        }
    };
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{:?}", self)]
struct Shouting {
    hello: String,
}
starlark_simple_value!(Shouting);

#[starlark_value(type = "shouting")]
impl<'v> StarlarkValue<'v> for Shouting {
    shout_repr!();
}

/// `repr` and `str` are the method an item macro defines, not the `Display` they are of a
/// block that defines neither.
#[test]
fn test_repr_from_item_macro() {
    let mut a = Assert::new();
    a.globals_add(|builder| {
        builder.set(
            "shouting",
            Shouting {
                hello: "world".to_owned(),
            },
        )
    });

    a.eq("repr(shouting)", "\"WORLD\"");
    a.eq("str(shouting)", "\"WORLD\"");
}
