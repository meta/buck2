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
use serde::Serialize;
use starlark_derive::NoSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::assert::Assert;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::docs::DocParam;
use crate::environment::GlobalsBuilder;
use crate::environment::Methods;
use crate::environment::MethodsBuilder;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::eval::runtime::params::display::PARAM_FMT_OPTIONAL;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::dict::DictType;
use crate::values::list::ListType;
use crate::values::none::NoneType;
use crate::values::tuple::TupleType;

#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    NoSerialize,
    ProvidesStaticType,
    StarlarkPagable
)]
#[display("input")]
struct InputTypeRepr;
#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    NoSerialize,
    ProvidesStaticType,
    StarlarkPagable
)]
#[display("output")]
struct OutputTypeRepr;

#[starlark_value(type = "input")]
impl<'v> StarlarkValue<'v> for InputTypeRepr {}

#[starlark_value(type = "output")]
impl<'v> StarlarkValue<'v> for OutputTypeRepr {}

#[starlark_module]
#[starlark_types(InputTypeRepr as Input, OutputTypeRepr as Output)]
fn globals(builder: &mut GlobalsBuilder) {
    #[allow(unused_variables)]
    fn simple<'v>(
        arg_int: i32,
        arg_bool: bool,
        arg_vec: ValueOf<'v, ListType<&'v str>>,
        arg_dict: ValueOf<'v, DictType<String, (bool, i32)>>,
    ) -> anyhow::Result<NoneType> {
        unimplemented!()
    }

    #[allow(unused_variables)]
    fn default_arg<'v>(
        arg1: Option<Value<'v>>,
        #[starlark(default = NoneType)] arg2: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Vec<String>> {
        unimplemented!()
    }

    #[allow(unused_variables)]
    fn args_kwargs<'v>(
        #[starlark(args)] args: ValueOf<'v, TupleType<Value<'v>>>,
        #[starlark(kwargs)] kwargs: Value<'v>,
    ) -> anyhow::Result<NoneType> {
        unimplemented!()
    }

    #[allow(unused_variables)]
    fn custom_types<'v>(
        arg1: StringValue<'v>,
        arg2: ValueTyped<'v, InputTypeRepr>,
        heap: Heap<'v>,
    ) -> anyhow::Result<ValueTyped<'v, OutputTypeRepr>> {
        unimplemented!()
    }

    #[allow(unused_variables)]
    fn pos_named(arg1: i32, #[starlark(require = named)] arg2: i32) -> anyhow::Result<i32> {
        unimplemented!()
    }

    #[allow(unused_variables)]
    fn with_arguments(args: &Arguments) -> anyhow::Result<i32> {
        unimplemented!()
    }
}

/// Test that a Rust starlark_module produces the right documentation.

#[test]
fn test_rustdoc() {
    let got = GlobalsBuilder::new().with(globals).build();
    let mut a = Assert::new();
    a.globals_add(globals);
    let expected = a.pass_module(r#"
def args_kwargs(*args, **kwargs: typing.Any) -> None: pass
def custom_types(arg1: str, arg2: Input) -> Output: pass
def default_arg(arg1 = "_", arg2: typing.Any = None) -> list[str]: pass
def pos_named(arg1: int, *, arg2: int) -> int: pass
def simple(arg_int: int, arg_bool: bool, arg_vec: list[str], arg_dict: dict[str, (bool, int)]) -> None: pass
def with_arguments(*args, **kwargs) -> int: pass
"#);

    let expected = expected.documentation().members;
    let mut got = got.documentation().members;

    got.shift_remove("Input");
    got.shift_remove("Output");

    assert_eq!(expected.len(), got.len());
    for (name, mut expected) in expected {
        if &name == "default_arg" {
            // `Option<Foo>` args in native functions are special magic and have behavior that can't
            // be replicated with normal functions
            let DocItem::Member(DocMember::Function(expected)) = &mut expected else {
                unreachable!()
            };
            let DocParam { default_value, .. } = expected.params.doc_params_mut().next().unwrap();
            *default_value = Some(PARAM_FMT_OPTIONAL.to_owned());
        }
        // Comparing one at a time produces more useful error messages
        assert_eq!(&expected, got.get(&name).unwrap());
    }
}

#[derive(
    ProvidesStaticType,
    Debug,
    Display,
    Allocative,
    Serialize,
    StarlarkPagable
)]
#[display("obj")]
struct Obj;

starlark::methods_static!(OBJ_METHODS = object);

#[starlark_value(type = "obj")]
impl<'v> StarlarkValue<'v> for Obj {
    fn get_methods() -> Option<&'static Methods> {
        Some(OBJ_METHODS.methods())
    }
}

/// These are where the module docs go
#[starlark_module]
fn object(builder: &mut MethodsBuilder) {
    /// Docs for func1
    ///
    /// # Arguments
    ///     * `foo`: Docs for foo
    ///
    /// # Returns
    /// The string 'func1'
    fn func1<'v>(this: Value<'v>, foo: String) -> anyhow::Result<String> {
        let _ignore = (this, foo);
        Ok("func1".to_owned())
    }
}

#[test]
fn inner_object_functions_have_docs() {
    Heap::temp(|heap| {
        let obj = heap.alloc_simple(Obj);
        let item = obj
            .get_attr("func1", heap)
            .unwrap()
            .unwrap()
            .documentation();

        match item {
            DocItem::Member(DocMember::Function(item)) => {
                assert_eq!(item.docs.unwrap().summary, "Docs for func1");
            }
            _ => panic!("Expected function: {item:#?}"),
        }
    });
}

#[starlark_module]
fn module(builder: &mut GlobalsBuilder) {
    const MAGIC: i32 = 42;

    /// Docs for func1
    ///
    /// # Arguments
    ///     * `foo`: Docs for foo
    ///
    /// # Returns
    /// The string 'func1'
    fn func1(foo: String) -> anyhow::Result<String> {
        let _ignore = foo;
        Ok("func1".to_owned())
    }
}

#[test]
fn inner_module_functions_have_docs() {
    let item = GlobalsBuilder::new()
        .with(module)
        .build()
        .get_ref("func1")
        .unwrap()
        .value()
        .documentation();

    match item {
        DocItem::Member(DocMember::Function(item)) => {
            assert_eq!(item.docs.unwrap().summary, "Docs for func1");
        }
        _ => panic!("Expected function: {item:#?}"),
    }
}
