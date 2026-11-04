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

use std::cell::Cell;

use starlark_derive::starlark_module;

use crate as starlark;
use crate::assert::Assert;
use crate::environment::GlobalsBuilder;
use crate::tests::bc::golden::bc_golden_test;
use crate::values::Value;

#[test]
fn test_methods_invoked_speculatively() {
    bc_golden_test(
        "speculative_exec_methods_invoked_speculatively",
        r#"
def test():
    return "foo".startswith("f")
"#,
    )
}

#[test]
fn test_format_speculatively_before_format_instr() {
    bc_golden_test(
        "speculative_exec_format_speculatively_before_format_instr",
        r#"
def test():
    # Test this expression is compiled to constant, not to `FormatOne` instruction.
    return "x{}y".format(1)
"#,
    );
}

#[test]
fn test_speculatively_inline_enum() {
    bc_golden_test(
        "speculative_exec_enum_inline",
        r#"
MyEnum = enum("red", "green", "blue")

def test(x):
    # Test there is no enum evaluation.
    return x == MyEnum("red")
"#,
    );
}

#[test]
fn test_min_max_key_not_invoked_speculatively() {
    thread_local! {
        static KEY_CALLS: Cell<i32> = const { Cell::new(0) };
    }

    #[starlark_module]
    fn natives(builder: &mut GlobalsBuilder) {
        fn counted_key<'v>(#[starlark(require = pos)] x: Value<'v>) -> anyhow::Result<Value<'v>> {
            KEY_CALLS.with(|c| c.set(c.get() + 1));
            Ok(x)
        }

        fn key_calls() -> anyhow::Result<i32> {
            Ok(KEY_CALLS.with(|c| c.get()))
        }
    }

    let mut a = Assert::new();
    a.globals_add(natives);
    a.pass(
        r#"
start = key_calls()

def f():
    return max((1, 2), key = counted_key), min((1, 2), key = counted_key)

# `key` can be any callable, so it must only run when `f` is called,
# not while `f` is compiled.
assert_eq(start, key_calls())
assert_eq((2, 1), f())
assert_eq((2, 1), f())
assert_eq(start + 8, key_calls())
"#,
    );
}
