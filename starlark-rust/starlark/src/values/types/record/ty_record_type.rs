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
use starlark_derive::StarlarkPagable;
use triomphe::Arc;

use crate as starlark;
use crate::eval::runtime::params::spec::ParametersSpecPrototype;
use crate::typing::Ty;

#[derive(Allocative, Debug, StarlarkPagable)]
#[doc(hidden)]
pub struct TyRecordData {
    /// Name of the record type.
    pub(crate) name: String,
    /// Type of record instance.
    #[starlark_pagable(pagable)]
    pub(crate) ty_record: Ty,
    /// Type of record type.
    #[starlark_pagable(pagable)]
    pub(crate) ty_record_type: Ty,
    /// The constructor's signature, which has no default values: the fields' defaults are read
    /// from the fields at call time. Building the signature is expensive enough to show up in
    /// profiles, so it is built once here and instantiated at the brand of each call.
    #[starlark_pagable(pagable)]
    pub(crate) parameter_spec_prototype: Arc<ParametersSpecPrototype>,
}

#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_good() {
        assert::pass(
            r#"
MyRec = record(x = int)

def foo(x: MyRec): pass

foo(MyRec(x = 1))
        "#,
        );
    }

    #[test]
    fn test_fail_compile_time() {
        assert::fail_golden(
            "src/values/types/record/ty_record_type/fail_compile_time.golden",
            r#"
MyRec = record(x = int)
WrongRec = record(x = int)

def foo(x: MyRec): pass

def bar():
    foo(WrongRec(x = 1))
        "#,
        );
    }

    #[test]
    fn test_fail_runtime_time() {
        assert::fail_golden(
            "src/values/types/record/ty_record_type/fail_runtime_time.golden",
            r#"
MyRec = record(x = int)
WrongRec = record(x = int)

def foo(x: MyRec): pass

noop(foo)(WrongRec(x = 1))
        "#,
        );
    }

    #[test]
    fn test_record_instance_typechecker_ty() {
        assert::pass(
            r#"
MyRec = record(x = int)
X = MyRec(x = 1)

def foo() -> MyRec:
    # This fails if record instance does not override `typechecker_ty`.
    return X
"#,
        );
    }

    #[test]
    fn test_typecheck_field_pass() {
        assert::pass(
            r#"
MyRec = record(x = int, y = int)

def f(rec: MyRec) -> int:
    return rec.x + rec.y

assert_eq(f(MyRec(x = 1, y = 2)), 3)
"#,
        );
    }

    #[test]
    fn test_typecheck_field_fail() {
        assert::fail_golden(
            "src/values/types/record/ty_record_type/typecheck_field_fail.golden",
            r#"
MyRec = record(x = int, y = int)

def f(rec: MyRec) -> int:
    return rec.z
"#,
        );
    }

    #[test]
    fn test_typecheck_record_type_call() {
        assert::fail_golden(
            "src/values/types/record/ty_record_type/typecheck_record_type_call.golden",
            r#"
MyRec = record(x = int)

def test():
    MyRec(x = "")
"#,
        );
    }
}
