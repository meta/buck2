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
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::AllocDict;
use crate::values::dict::StarlarkDict;
use crate::values::int::IntView;
use crate::values::list::AllocList;
use crate::values::list::StarlarkList;
use crate::values::set::SetRef;
use crate::values::set::SetType;
use crate::values::string::StarlarkStr;
use crate::values::tuple::AllocTuple;
use crate::values::tuple::StarlarkTuple;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::typing::StarlarkNever;

#[derive(UnpackValue, Eq, PartialEq, Debug)]
enum EmptyEnum {}

#[derive(UnpackValue, Debug)]
enum JustInt<'v> {
    Int(IntView<'v>),
}

#[derive(UnpackValue, Debug)]
enum IntOrStr<'v> {
    Int(IntView<'v>),
    Str(ValueOf<'v, StarlarkStr>),
}

#[derive(UnpackValue, Debug)]
struct TransparentIntOrStr<'v>(IntOrStr<'v>);

#[test]
fn test_starlark_type_repr() {
    assert_eq!(
        StarlarkNever::starlark_type_repr(),
        EmptyEnum::starlark_type_repr()
    );

    assert_eq!(IntView::starlark_type_repr(), JustInt::starlark_type_repr());

    assert_eq!(
        Either::<IntView, StarlarkStr>::starlark_type_repr(),
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

#[derive(UnpackValue)]
enum References<'v> {
    Str(&'v str),
    List(StarlarkList<'v>),
    Tuple(StarlarkTuple<'v>),
    Dict(StarlarkDict<'v>),
    Set(SetRef<'v>),
}

#[test]
fn test_reference_payloads() {
    assert_eq!(
        Either::<
            Either<Either<StarlarkStr, StarlarkList>, StarlarkTuple>,
            Either<StarlarkDict, SetType<Value>>,
        >::starlark_type_repr(),
        References::starlark_type_repr()
    );

    Heap::temp(|heap| {
        let show = |v: Value| match References::unpack_value_opt(v) {
            Some(References::Str(s)) => format!("str {s}"),
            Some(References::List(l)) => format!("list {}", l.len()),
            Some(References::Tuple(t)) => format!("tuple {}", t.len()),
            Some(References::Dict(d)) => format!("dict {}", d.len()),
            Some(References::Set(_)) => "set".to_owned(),
            None => "other".to_owned(),
        };
        assert_eq!("str abc", show(heap.alloc("abc").to_value()));
        assert_eq!("list 2", show(heap.alloc(AllocList([1, 2])).to_value()));
        assert_eq!(
            "tuple 3",
            show(heap.alloc(AllocTuple([1, 2, 3])).to_value())
        );
        assert_eq!("dict 1", show(heap.alloc(AllocDict([(1, 2)])).to_value()));
        assert_eq!("other", show(Value::testing_new_int(1)));
    });
    let set = crate::assert::pass("set([1, 2])");
    assert!(matches!(
        References::unpack_value_opt(set.as_ref().value()),
        Some(References::Set(s)) if s.iter().count() == 2
    ));
}
