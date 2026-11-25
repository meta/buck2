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
use crate::values::none::NoneOr;
use crate::values::none::NoneType;
use crate::values::set::StarlarkSet;
use crate::values::string::StarlarkStr;
use crate::values::tuple::AllocTuple;
use crate::values::tuple::StarlarkTuple;
use crate::values::type_expr::Accepts;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::typing::StarlarkIter;
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

#[derive(UnpackValue)]
#[expect(dead_code, reason = "only the type is used")]
enum IntOrStrWhere<'v>
where
    'v: 'v,
{
    Int(IntView<'v>),
    Str(&'v str),
}

/// Compiles if the type expression `A` takes every value of the type expression `T`.
fn takes<'v, A: Accepts<'v, T, Path>, T, Path>() {}

#[test]
fn test_accepts() {
    // A type takes itself.
    takes::<IntView, IntView, _>();
    // A union takes what an alternative takes.
    takes::<NoneOr<IntView>, NoneType, _>();
    takes::<NoneOr<IntView>, IntView, _>();
    takes::<Either<IntView, StarlarkStr>, StarlarkStr, _>();
    takes::<NoneOr<Either<IntView, StarlarkStr>>, StarlarkStr, _>();
    takes::<IntOrStr, IntView, _>();
    takes::<IntOrStr, StarlarkStr, _>();
    takes::<TransparentIntOrStr, StarlarkStr, _>();
    takes::<NoneOr<IntOrStrWhere>, StarlarkStr, _>();
    // The empty list and the empty dict.
    takes::<StarlarkList<StarlarkStr>, StarlarkList<StarlarkNever>, _>();
    takes::<NoneOr<StarlarkList<StarlarkStr>>, StarlarkList<StarlarkNever>, _>();
    takes::<StarlarkIter<Value>, StarlarkList<StarlarkNever>, _>();
    takes::<StarlarkDict<StarlarkStr, IntView>, StarlarkDict<StarlarkNever, StarlarkNever>, _>();
}

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
enum StrOrContainer<'v> {
    Str(&'v str),
    List(StarlarkList<'v>),
    Tuple(StarlarkTuple<'v>),
    Dict(StarlarkDict<'v>),
    Set(StarlarkSet<'v>),
}

#[test]
fn test_str_and_container_payloads() {
    assert_eq!(
        Either::<
            Either<Either<StarlarkStr, StarlarkList>, StarlarkTuple>,
            Either<StarlarkDict, StarlarkSet>,
        >::starlark_type_repr(),
        StrOrContainer::starlark_type_repr()
    );

    Heap::temp(|heap| {
        let show = |v: Value| match StrOrContainer::unpack_value_opt(v) {
            Some(StrOrContainer::Str(s)) => format!("str {s}"),
            Some(StrOrContainer::List(l)) => format!("list {}", l.len()),
            Some(StrOrContainer::Tuple(t)) => format!("tuple {}", t.len()),
            Some(StrOrContainer::Dict(d)) => format!("dict {}", d.len()),
            Some(StrOrContainer::Set(_)) => "set".to_owned(),
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
        StrOrContainer::unpack_value_opt(set.as_ref().value()),
        Some(StrOrContainer::Set(s)) if s.iter().count() == 2
    ));
}
