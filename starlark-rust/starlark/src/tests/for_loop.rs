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

#[test]
fn test_for_loop_bug_1() {
    assert::pass(
        r#"
def test(x):
    for i in x:
        # This should release mutation lock on `x`.
        return i

l = [1]
test(l)
l.append(1)
"#,
    );
}

/// A `for` statement or a comprehension over `""` fails like one over any other string:
/// strings have a length but are not iterable, so they are not removed as empty iterables.
#[test]
fn test_for_over_empty_string_fails() {
    assert::fail("[x for x in '']", "not supported on type `string`");
    assert::fail("{x: 1 for x in ''}", "not supported on type `string`");
    assert::eq("[x for x in ()]", "[]");
    assert::eq("[x for x in []]", "[]");
    let mut a = Assert::new();
    a.disable_static_typechecking();
    a.fail(
        "def f():\n    for x in '':\n        pass\n    return 1\nf()",
        "not supported on type `string`",
    );
    a.module(
        "lib.star",
        "E = 'x'\nE = ''\ndef f():\n    for x in E:\n        pass\n    return 1\n",
    );
    a.fail(
        "load('lib.star', 'f')\nf()",
        "not supported on type `string`",
    );
}
