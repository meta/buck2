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

use allocative::Allocative;
use derive_more::Display;
use serde::Serialize;
use serde::Serializer;
use starlark_derive::Freeze;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::Trace;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::docs::DocMember;
use crate::docs::DocString;
use crate::docs::DocStringKind;
use crate::docs::DocType;
use crate::environment::Methods;
use crate::environment::MethodsBuilder;
use crate::starlark_complex_value;
use crate::starlark_simple_value;
use crate::values::StarlarkValue;
use crate::values::Value;

/// Main module docs
#[starlark_module]
fn object_docs_1(_: &mut MethodsBuilder) {
    /// Returns the string "foo"
    #[starlark(attribute)]
    fn foo(this: &TestExample) -> starlark::Result<String> {
        Ok("foo".to_owned())
    }
}

#[derive(
    Debug,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
struct TestExample {}

starlark_simple_value!(TestExample);

starlark::methods_static!(TEST_EXAMPLE_METHODS = object_docs_1);

#[starlark_value(type = "TestExample")]
impl<'v> StarlarkValue<'v> for TestExample {
    fn get_methods() -> Option<&'static Methods>
    where
        Self: Sized,
    {
        Some(TEST_EXAMPLE_METHODS.methods())
    }
}

#[derive(
    Clone,
    Debug,
    Display,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
struct ComplexTestExample<'v>(Value<'v>);

impl<'v> Serialize for ComplexTestExample<'v> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

starlark_complex_value!(ComplexTestExample);

starlark::methods_static!(COMPLEX_TEST_EXAMPLE_METHODS = object_docs_1);

#[starlark_value(type = "ComplexTestExample", alloc = by_hand)]
impl<'v> StarlarkValue<'v> for ComplexTestExample<'v>
where
    Self: ProvidesStaticType<'v>,
{
    fn get_methods() -> Option<&'static Methods>
    where
        Self: Sized,
    {
        Some(COMPLEX_TEST_EXAMPLE_METHODS.methods())
    }
}

#[test]
fn test_derive_docs() {
    let obj = DocType::from_starlark_value::<TestExample>();

    assert_eq!(
        DocString::from_docstring(DocStringKind::Rust, "Main module docs"),
        obj.docs
    );
    assert_eq!(
        DocString::from_docstring(DocStringKind::Rust, "Returns the string \"foo\""),
        obj.members
            .iter()
            .find_map(|(name, m)| match m {
                DocMember::Property(p) if name == "foo" => Some(p.docs.clone()),
                _ => None,
            })
            .unwrap()
    );
}

#[test]
fn test_derive_docs_on_complex_values() {
    let complex_obj = DocType::from_starlark_value::<ComplexTestExample<'static>>();

    assert_eq!(
        DocString::from_docstring(DocStringKind::Rust, "Main module docs"),
        complex_obj.docs
    );
    assert_eq!(
        DocString::from_docstring(DocStringKind::Rust, "Returns the string \"foo\""),
        complex_obj
            .members
            .iter()
            .find_map(|(name, m)| match m {
                DocMember::Property(p) if name == "foo" => Some(p.docs.clone()),
                _ => None,
            })
            .unwrap()
    );
}
