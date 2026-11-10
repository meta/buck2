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

use either::Either;

use crate as starlark;
use crate::const_frozen_string;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::int::IntView;
use crate::values::string::StarlarkStr;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::typing::StarlarkNever;

#[derive(StarlarkTypeRepr, UnpackValue, Eq, PartialEq, Debug)]
enum EmptyEnum {}

#[derive(StarlarkTypeRepr, UnpackValue, Debug)]
enum JustInt<'v> {
    Int(IntView<'v>),
}

#[derive(StarlarkTypeRepr, UnpackValue, Debug)]
enum IntOrStr<'v> {
    Int(IntView<'v>),
    Str(ValueOf<'v, StarlarkStr>),
}

#[derive(StarlarkTypeRepr, UnpackValue, Debug)]
struct TransparentIntOrStr<'v>(IntOrStr<'v>);

#[test]
fn test_starlark_type_repr() {
    assert_eq!(
        StarlarkNever::starlark_type_repr(),
        EmptyEnum::starlark_type_repr()
    );

    assert_eq!(i32::starlark_type_repr(), JustInt::starlark_type_repr());

    assert_eq!(
        Either::<i32, String>::starlark_type_repr(),
        IntOrStr::starlark_type_repr()
    );

    assert_eq!(
        IntOrStr::starlark_type_repr(),
        TransparentIntOrStr::starlark_type_repr(),
    );
}

fn show(v: IntOrStr) -> String {
    match v {
        IntOrStr::Int(i) => format!("int {}", i.to_value().unpack_i32().unwrap()),
        IntOrStr::Str(s) => format!("str {}", s.get().as_str()),
    }
}

#[test]
fn test_unpack_value() {
    let JustInt::Int(i) = JustInt::unpack_value(Value::testing_new_int(17))
        .unwrap()
        .unwrap();
    assert_eq!(Some(17), i.to_value().unpack_i32());
    assert!(JustInt::unpack_value(Value::new_none()).unwrap().is_none());

    assert_eq!(
        "int 19",
        show(
            IntOrStr::unpack_value(Value::testing_new_int(19))
                .unwrap()
                .unwrap()
        ),
    );
    assert_eq!(
        "str abc",
        show(
            IntOrStr::unpack_value(const_frozen_string!("abc").at().to_value())
                .unwrap()
                .unwrap()
        ),
    );
    assert!(IntOrStr::unpack_value(Value::new_none()).unwrap().is_none());

    let TransparentIntOrStr(inner) = TransparentIntOrStr::unpack_value(Value::testing_new_int(19))
        .unwrap()
        .unwrap();
    assert_eq!("int 19", show(inner));
}
