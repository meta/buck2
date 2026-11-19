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
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::assert::Assert;
use crate::environment::GlobalsBuilder;
use crate::values::StarlarkValue;
use crate::values::function::NativeFunction;

#[derive(
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("foo")]
struct Foo;

#[starlark_value(type = "Foo")]
impl<'v> StarlarkValue<'v> for Foo {}

#[starlark_module]
fn type_annotation_functions(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = Foo)]
    fn foo(x: i32) -> anyhow::Result<i32> {
        Ok(x)
    }
}

/// Guard against a future type constructor silently regressing.
///
/// A global is a type constructor when it is declared `#[starlark(as_type = ...)]`,
/// which is what gives it a runtime `.type` attribute. Nothing in the type system
/// enforces that such a global's `TyCustomFunctionImpl` also reports `is_type()`, and
/// getting that wrong is invisible until someone writes `X.type` inside a `def`. This
/// walks every global and checks the two agree, so adding a fifth type constructor
/// without overriding `is_type()` fails here rather than in a user's build file.
#[test]
fn test_every_as_type_global_has_a_typecheckable_type_attr() {
    let globals = GlobalsBuilder::extended().build();
    let mut checked = Vec::new();
    for (name, value) in globals.iter() {
        let Some(func) = value.value().downcast_ref::<NativeFunction>() else {
            continue;
        };
        if func.as_type.is_none() {
            continue;
        }
        // `.type` works at runtime for this global, so it must typecheck too.
        let a = Assert::new();
        a.pass(&format!("def _f():\n    return {name}.type\n"));
        checked.push(name.to_owned());
    }
    // If this trips, the walk stopped finding type constructors and is no longer
    // guarding anything.
    assert!(
        checked.len() >= 4,
        "expected to find several `as_type` globals, found {checked:?}"
    );
}

#[test]
fn test_type_annotation() {
    let mut a = Assert::new();
    a.globals_add(type_annotation_functions);
    a.eq("'Foo'", "foo.type");
    // Same, but from inside a `def`, which is statically typechecked.
    a.eq("'Foo'", "def f():\n    return foo.type\nf()");
}

/// `.type` must resolve for every global that is declared `as_type = ...`, not just
/// those backed by `TyFunction`. `struct` and `namespace` have hand-written
/// `TyCustomFunctionImpl`s, so they need `is_type()` overridden to match.
#[test]
fn test_type_attr_on_hand_written_type_constructors() {
    let a = Assert::new();
    a.eq("'struct'", "def f():\n    return struct.type\nf()");
    a.eq("'namespace'", "def f():\n    return namespace.type\nf()");
    a.eq("'list'", "def f():\n    return list.type\nf()");
}
