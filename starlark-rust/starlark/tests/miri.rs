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

//! Focused regressions for Starlark operations that previously failed under Miri.
//! Each test covers an unsafe layout or interpreter path fixed in the preceding stack.
//!
//! This is deliberately a standalone test crate rather than a module under `src/tests`: the
//! latter would compile the complete unit-test harness, which is far too expensive under Miri.
//! starlark-rust's GitHub CI runs it with `cargo miri test --test miri`; buck2's CI runs it as
//! `//buck2/starlark-rust/starlark:miri_regressions-miri`.
//!
//! This file is compiled both from the `starlark-rust` export, where the `pagable` feature
//! is off, and from the `buck2` workspace, where `fbcode/buck2/Cargo.toml` turns it on for
//! every crate. Use only APIs that exist under both: notably `Module::freeze_named` rather
//! than `Module::freeze`, which is `#[cfg(not(feature = "pagable"))]`.

use starlark::environment::Globals;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::AstModule;
use starlark::syntax::Dialect;
use starlark::values::FrozenHeapName;
use starlark::values::Heap;
use starlark::values::Value;
use starlark::values::list::AllocList;
use starlark::values::list::ListRef;
use starlark::values::tuple::AllocTuple;
use starlark::values::tuple::TupleRef;

#[test]
fn hello_world() {
    let ast = AstModule::parse(
        "miri.star",
        r#"
def hello():
    return "hello world"

small = str(1e10)
large = str(1e300)
hello()
"#
        .to_owned(),
        &Dialect::Standard,
    )
    .unwrap();
    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        let result = eval.eval_module(ast, &Globals::standard()).unwrap();
        assert_eq!(result.unpack_str(), Some("hello world"));
        assert_eq!(module.get("small").unwrap().unpack_str(), Some("1e+10"));
        assert_eq!(module.get("large").unwrap().unpack_str(), Some("1e+300"));
    });
}

#[test]
fn none_type() {
    assert_eq!(Value::default().get_type(), "NoneType");
}

#[test]
fn list_allocation() {
    Heap::temp(|heap| {
        let value = heap.alloc(AllocList([1i32, 2, 3])).to_value();
        let list = ListRef::from_value(value).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[1].unpack_i32(), Some(2));
    });
}

#[test]
fn tuple_allocation() {
    Heap::temp(|heap| {
        let value = heap.alloc(AllocTuple([1i32, 2])).to_value();
        let tuple = TupleRef::from_value(value).unwrap();
        assert_eq!(tuple.content().len(), 2);
        assert_eq!(tuple.content()[1].unpack_i32(), Some(2));
    });
}

#[test]
fn single_character_module_name() {
    Module::with_temp_heap(|module| {
        let value = module.heap().alloc("value").to_value();
        module.set("x", value);
        assert_eq!(module.get("x").unwrap().unpack_str(), Some("value"));
        module.freeze_named(FrozenHeapName::user("miri")).unwrap();
    });
}
