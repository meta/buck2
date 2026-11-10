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

use allocative::Allocative;
use either::Either;
use itertools::Itertools;
use starlark_derive::NoSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::assert::Assert;
use crate::environment::GlobalsBuilder;
use crate::starlark_simple_value;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::DictType;
use crate::values::list::ListType;
use crate::values::set::SetType;
use crate::values::types::int::view::IntView;

// TODO(nmj): Figure out default values here. ValueOf<i32> = 5 should work.
#[starlark_module]
fn validate_module(builder: &mut GlobalsBuilder) {
    fn with_int<'v>(v: ValueOf<'v, IntView<'v>>) -> anyhow::Result<(Value<'v>, String)> {
        Ok((v.to_value(), format!("{}", v.get())))
    }
    fn with_int_list<'v>(
        v: ValueOf<'v, ListType<IntView<'v>>>,
    ) -> anyhow::Result<(Value<'v>, String)> {
        let repr = v.try_to_vec().unwrap().iter().join(", ");
        Ok((v.to_value(), repr))
    }
    fn with_list_list<'v>(
        v: ValueOf<'v, ListType<ListType<IntView<'v>>>>,
    ) -> anyhow::Result<(Value<'v>, String)> {
        let repr = v
            .iter()
            .map(Result::unwrap)
            .map(|l| l.try_to_vec().unwrap().iter().join(", "))
            .join(" + ");
        Ok((v.to_value(), repr))
    }
    fn with_dict_list<'v>(
        v: ValueOf<'v, ListType<DictType<IntView<'v>, IntView<'v>>>>,
    ) -> anyhow::Result<(Value<'v>, String)> {
        let repr = v
            .iter()
            .map(Result::unwrap)
            .map(|l| {
                l.try_items()
                    .map(Result::unwrap)
                    .map(|(k, v)| format!("{k}: {v}"))
                    .join(", ")
            })
            .join(" + ");
        Ok((v.to_value(), repr))
    }
    fn with_int_dict<'v>(
        v: ValueOf<'v, DictType<IntView<'v>, IntView<'v>>>,
    ) -> anyhow::Result<(Value<'v>, String)> {
        let repr = v
            .try_items()
            .map(Result::unwrap)
            .map(|(k, v)| format!("{k}: {v}"))
            .join(" + ");
        Ok((v.to_value(), repr))
    }
    fn with_list_dict<'v>(
        v: ValueOf<'v, DictType<IntView<'v>, ListType<IntView<'v>>>>,
    ) -> anyhow::Result<(Value<'v>, String)> {
        let repr = v
            .iter()
            .map(Result::unwrap)
            .map(|(k, v)| format!("{}: {}", k.get(), v.try_to_vec().unwrap().iter().join(", ")))
            .join(" + ");
        Ok((v.to_value(), repr))
    }
    fn with_dict_dict<'v>(
        v: ValueOf<'v, DictType<IntView<'v>, DictType<IntView<'v>, IntView<'v>>>>,
    ) -> anyhow::Result<(Value<'v>, String)> {
        let repr = v
            .iter()
            .map(Result::unwrap)
            .map(|(k, v)| {
                let inner_repr = v
                    .try_items()
                    .map(Result::unwrap)
                    .map(|(k2, v2)| format!("{k2}:{v2}"))
                    .join(", ");
                format!("{}: {inner_repr}", k.get())
            })
            .join(" + ");
        Ok((v.to_value(), repr))
    }
    fn with_either<'v>(
        v: Either<i32, Either<String, ValueOf<'v, ListType<IntView<'v>>>>>,
    ) -> anyhow::Result<String> {
        match v {
            Either::Left(i) => Ok(i.to_string()),
            Either::Right(nested) => match nested {
                Either::Left(s) => Ok(s),
                Either::Right(l) => Ok(l.to_value().to_repr()),
            },
        }
    }
    fn with_set<'v>(v: ValueOf<'v, SetType<IntView<'v>>>) -> anyhow::Result<(Value<'v>, String)> {
        let repr = v.try_items().map(Result::unwrap).join(", ");
        Ok((v.to_value(), repr))
    }
}

/// A type with no `UnpackValue`: its own name is its type argument.
#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("nominal")]
struct Nominal;
starlark_simple_value!(Nominal);

#[starlark_value(type = "nominal")]
impl<'v> StarlarkValue<'v> for Nominal {}

#[starlark_module]
fn nominal_module(builder: &mut GlobalsBuilder) {
    fn nominal() -> anyhow::Result<Nominal> {
        Ok(Nominal)
    }

    fn with_nominal<'v>(v: ValueOf<'v, Nominal>) -> anyhow::Result<String> {
        let by_deref: &Nominal = &v;
        assert!(std::ptr::eq(by_deref, v.as_ref()));
        Ok(by_deref.to_string())
    }

    fn with_nominal_list<'v>(v: ValueOf<'v, ListType<Nominal>>) -> anyhow::Result<usize> {
        Ok(v.iter().count())
    }
}

#[test]
fn test_nominal_type_argument() {
    let mut a = Assert::new();
    a.globals_add(nominal_module);
    a.eq("'nominal'", "with_nominal(nominal())");
    a.eq("2", "with_nominal_list([nominal(), nominal()])");
    a.fail("with_nominal(1)", BAD);
    a.fail("with_nominal_list([nominal(), 1])", BAD);
}

// The standard error these raise on incorrect types
const BAD: &str = "Type of parameter";

#[test]
fn test_value_of() {
    let mut a = Assert::new();
    a.globals_add(validate_module);
    a.eq("(1, '1')", "with_int(1)");
    a.fail("with_int(noop(None))", BAD);
}

#[test]
fn test_list_of() {
    let mut a = Assert::new();
    a.globals_add(validate_module);
    a.eq("([1, 2, 3], '1, 2, 3')", "with_int_list([1, 2, 3])");
    a.fail("with_int_list(noop(1))", BAD);
    a.fail("with_int_list(noop([1, 'foo']))", BAD);
    a.fail("with_int_list(noop([[]]))", BAD);

    a.eq(
        "([[1, 2], [3]], '1, 2 + 3')",
        "with_list_list([[1, 2], [3]])",
    );

    let expected = r#"([{1: 2, 3: 4}, {5: 6}], "1: 2, 3: 4 + 5: 6")"#;
    let test = r#"with_dict_list([{1: 2, 3: 4}, {5: 6}])"#;
    a.eq(expected, test);
}

#[test]
fn test_dict_of() {
    let mut a = Assert::new();
    a.globals_add(validate_module);
    a.eq("({1: 2}, '1: 2')", "with_int_dict({1: 2})");
    a.fail(r#"with_int_dict(noop(1))"#, BAD);
    a.fail(r#"with_int_dict(noop({1: "str"}))"#, BAD);
    a.fail(r#"with_int_dict(noop({1: {}}))"#, BAD);

    let expected = r#"({1: [2, 3], 4: [5]}, "1: 2, 3 + 4: 5")"#;
    let test = r#"with_list_dict({1: [2, 3], 4: [5]})"#;
    a.eq(expected, test);

    let expected = r#"({1: {2: 3, 4: 5}, 6: {7: 8}}, "1: 2:3, 4:5 + 6: 7:8")"#;
    let test = r#"with_dict_dict({1: {2: 3, 4: 5}, 6: {7: 8}})"#;
    a.eq(expected, test);
}

#[test]
fn test_either_of() {
    let mut a = Assert::new();
    a.globals_add(validate_module);
    a.eq("'2'", "with_either(2)");
    a.eq("'[2, 3]'", "with_either([2,3])");
    a.eq("'s'", "with_either('s')");
    a.fail("with_either(noop(None))", BAD);
    a.fail("with_either(noop({}))", BAD);
}

#[test]
fn test_set_of() {
    let mut a = Assert::new();
    a.globals_add(validate_module);
    a.eq("(set([2]), '2')", "with_set(set([2]))");
    a.eq("(set([2, 3]), '2, 3')", "with_set(set([2,3]))");
    a.fail("with_set(noop(None))", BAD);
    a.fail("with_set(noop({}))", BAD);
}
