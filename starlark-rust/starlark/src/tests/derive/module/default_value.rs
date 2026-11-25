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
use crate::const_frozen_string;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::environment::GlobalsBuilder;
use crate::values::EmptyDict;
use crate::values::EmptyList;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::StarlarkDict;
use crate::values::list::StarlarkList;
use crate::values::none::NoneOr;
use crate::values::none::NoneType;

const NAMED: &str = "named";

#[starlark_module]
fn default_value_functions(globals: &mut GlobalsBuilder) {
    fn foo(#[starlark(default = 75)] x: i32) -> anyhow::Result<i32> {
        Ok(x)
    }

    /// Integer literals in the forms a default has them.
    fn ints(
        #[starlark(default = -1)] negative: i32,
        #[starlark(default = 0o444)] octal: i32,
        #[starlark(default = 86400)] day: i32,
    ) -> anyhow::Result<String> {
        Ok(format!("{negative} {octal} {day}"))
    }

    /// One default of each kind that can be written.
    fn kinds<'v>(
        #[starlark(default = true)] flag: bool,
        #[starlark(default = NoneType)] none: NoneOr<i32>,
        #[starlark(default = 3)] int_or_none: NoneOr<i32>,
        #[starlark(default = "lit")] literal: &str,
        #[starlark(default = const_frozen_string!(NAMED))] constant: &str,
        #[starlark(default = EmptyList)] list: ValueOf<'v, StarlarkList<'v>>,
        #[starlark(default = EmptyDict)] dict: ValueOf<'v, StarlarkDict<'v>>,
        #[starlark(default = NoneType)] any: Value<'v>,
    ) -> anyhow::Result<String> {
        Ok(format!(
            "{flag} {none:?} {int_or_none:?} {literal} {constant} {} {} {any}",
            list.to_value(),
            dict.len(),
            none = none.into_option(),
            int_or_none = int_or_none.into_option(),
        ))
    }
}

#[test]
fn test_default_value() {
    let mut a = Assert::new();
    a.globals_add(default_value_functions);
    a.eq("74", "foo(74)");
    a.eq("75", "foo()");
}

#[test]
fn test_int_defaults() {
    let mut a = Assert::new();
    a.globals_add(default_value_functions);
    a.eq("'-1 292 86400'", "ints()");
}

/// An `i32` that is not a literal is a default where every `i32` is an inline int.
#[cfg(target_pointer_width = "64")]
#[starlark_module]
fn i32_default_functions(globals: &mut GlobalsBuilder) {
    fn least(#[starlark(default = i32::MIN)] x: i32) -> anyhow::Result<i32> {
        Ok(x)
    }
}

#[cfg(target_pointer_width = "64")]
#[test]
fn test_i32_default() {
    let mut a = Assert::new();
    a.globals_add(i32_default_functions);
    a.eq("-2147483648", "least()");
}

#[test]
fn test_default_of_each_kind() {
    let mut a = Assert::new();
    a.globals_add(default_value_functions);
    a.eq("'true None Some(3) lit named [] 0 None'", "kinds()");
    // An argument that is left out is the default passed.
    a.eq(
        "kinds()",
        "kinds(True, None, 3, 'lit', 'named', [], {}, None)",
    );
}

#[test]
fn test_default_is_documented_as_its_repr() {
    let globals = GlobalsBuilder::standard()
        .with(default_value_functions)
        .build();
    let DocItem::Member(DocMember::Function(kinds)) = globals
        .documentation()
        .members
        .shift_remove("kinds")
        .unwrap()
    else {
        panic!("`kinds` is a function");
    };
    let defaults: Vec<_> = kinds
        .params
        .pos_or_named
        .iter()
        .map(|p| p.default_value.as_deref().unwrap())
        .collect();
    assert_eq!(
        defaults,
        [
            "True",
            "None",
            "3",
            "\"lit\"",
            "\"named\"",
            "[]",
            "{}",
            "None"
        ],
    );
}
