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

use crate::assert;
use crate::assert::Assert;
use crate::tests::bc::golden::bc_golden_test;

#[test]
fn test_fold_list_add() {
    bc_golden_test("constant_folding_list_add", "def test(): return [1] + [2]");
}

#[test]
fn test_fold_list_add_too_large() {
    let mut a = Assert::new();
    a.module("a0.bzl", "def a0(): return [1]");
    for i in 1..100 {
        // `a_17()` is inlined into `a_18()`.
        // If inlining is not limited, this test will run for very long time
        // and eat up all the memory.
        let i_1 = i - 1;
        a.module(
            &format!("a{i}.bzl"),
            &format!(
                "load('a{i_1}.bzl', 'a{i_1}')\n\
                def a{i}(): return a{i_1}() + a{i_1}() + a{i_1}() + a{i_1}() + a{i_1}()"
            ),
        );
    }
}

/// `ExprCompiled::slice` folds `c[a:b:s]` only when every written part is a constant; a part
/// that is present but not constant disables folding, so the slice is evaluated at runtime.
#[test]
fn test_slice_fold_keeps_non_constant_part() {
    assert::eq("'abcdef'[noop(2):4:1]", "'cd'");
    assert::eq("'abcdef'[1:noop(3):1]", "'bc'");
    assert::eq("'abcdef'[1:3:noop(-1)]", "''");
    assert::eq("(1, 2, 3, 4, 5)[noop(3):5:1]", "(4, 5)");
    assert::fail("'abc'[noop('x'):2:1]", "");
    assert::fail("def g():\n    fail('boom')\n'abc'[g():2:1]", "boom");
    assert::eq("def f(i):\n    return 'abcdef'[i:4:1]\nf(2)", "'cd'");
    // Fully constant slices still fold, including ones with omitted parts.
    assert::eq("'abcdef'[1:4:2]", "'bd'");
    assert::eq("'abcdef'[::-1]", "'fedcba'");
    assert::eq("'abcdef'[2:]", "'cdef'");
    let mut a = Assert::new();
    a.module(
        "lib.star",
        "S = ''\nS = 'abcdef'\ndef f(i):\n    return S[i:4:1]\nBEFORE = f(2)\n",
    );
    a.eq("load('lib.star', 'BEFORE')\nBEFORE", "'cd'");
    a.eq("load('lib.star', 'f')\nf(2)", "'cd'");
}
