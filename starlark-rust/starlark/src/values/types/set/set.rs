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

use crate::environment::GlobalsBuilder;
use crate::values::set::StarlarkSet;

pub(crate) fn register_set(globals: &mut GlobalsBuilder) {
    globals.type_symbol::<StarlarkSet>();
}
#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_set_type_as_type_compile_time() {
        assert::fail(
            r"
def f_fail_ct(x: set[int]):
    return x

s = set(['not_int'])

f_fail_ct(s)
",
            //Is it actually runtime or compile time error?
            r#"Value `set(["not_int"])` of type `set` does not match the type annotation `set[int]` for argument `x`"#,
        );
    }

    #[test]
    fn test_return_set_type_as_type_compile_time() {
        assert::fail(
            r"
def f_fail_ct(x: str) -> set[int]:
    return set([x])

f_fail_ct('not_int')
",
            //Is it actually runtime or compile time error?
            r#"Value `set(["not_int"])` of type `set` does not match the type annotation `set[int]` for return type"#,
        );
    }

    #[test]
    fn test_set_type_as_type_run_time() {
        assert::fail(
            r"
def f_fail_rt(x: set[int]):
    return x

s = set(['not_int'])

noop(f_fail_rt)(s)
",
            r#"Value `set(["not_int"])` of type `set` does not match the type annotation `set[int]` for argument `x`"#,
        );
    }
}
