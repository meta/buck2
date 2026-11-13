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

use starlark_derive::starlark_module;

use crate as starlark;
use crate::assert::Assert;
use crate::environment::GlobalsBuilder;
use crate::environment::MethodsBuilder;
use crate::values::Heap;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::list_or_tuple::ListOrTupleType;
use crate::values::none::NoneType;
use crate::values::typing::StarlarkCallableChecked;

// The examples from the starlark_module documentation.
#[test]
fn test_starlark_module() {
    #[starlark_module]
    fn global(builder: &mut GlobalsBuilder) {
        fn cc_binary<'v>(
            name: &str,
            srcs: ValueOf<'v, ListOrTupleType<&'v str>>,
        ) -> anyhow::Result<String> {
            // real implementation may write it to a global variable
            let srcs = srcs.try_to_vec::<&str>().map_err(|e| e.into_anyhow())?;
            Ok(format!("{name:?} {srcs:?}"))
        }
    }

    let mut a = Assert::new();
    a.globals_add(global);
    let v = a.pass("cc_binary(name='star', srcs=['a.cc', 'b.cc'])");
    assert_eq!(
        v.by_ref(|v| v.unpack_str().unwrap().to_owned()),
        r#""star" ["a.cc", "b.cc"]"#
    );
}

#[test]
fn test_starlark_methods() {
    #[starlark_module]
    fn methods(builder: &mut MethodsBuilder) {
        fn r#enum<'v>(
            this: Value<'v>,
            #[starlark(require = named, default = 3)] index: i32,
            heap: Heap<'v>,
        ) -> anyhow::Result<StringValue<'v>> {
            Ok(heap.alloc_str(&format!("{this} {index}")))
        }
    }

    MethodsBuilder::new().with(methods).build();
}

#[test]
fn test_static_allowed() {
    #[starlark_module]
    fn globals(globals: &mut GlobalsBuilder) {
        fn test<'v>()
        -> anyhow::Result<ValueOf<'v, StarlarkCallableChecked<'v, (&'static str,), NoneType>>>
        {
            panic!()
        }
    }

    GlobalsBuilder::standard().with(globals).build();
}
