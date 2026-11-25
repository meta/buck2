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

#![cfg(test)]

use crate::assert;
use crate::collections::SmallMap;
use crate::values::Heap;
use crate::values::dict::Dict;
use crate::values::dict::DictMut;
use crate::values::dict::DictRef;
use crate::values::typing::type_compiled::compiled::TypeCompiled;
use crate::values::typing::type_compiled::matcher::TypeMatcher;
use crate::values::typing::type_compiled::matchers::IsAnyOfTwo;
use crate::values::typing::type_compiled::matchers::IsDictOf;
use crate::values::typing::type_compiled::matchers::IsInt;
use crate::values::typing::type_compiled::matchers::IsStr;

#[test]
fn test_types() {
    let a = assert::Assert::new();
    a.is_true(
        r#"
def f(i: int) -> bool:
    return i == 3
f(8) == False"#,
    );

    // If the types are either malformed or runtime errors, it should fail
    a.fail("def f(i: made_up):\n pass", "Variable");
    a.fail(
        "def f(i: fail('bad')):\n pass",
        "call expression is not allowed in type expression",
    );

    // Type errors should be caught in arguments
    a.fail(
        "def f_runtime(i: bool):\n pass\nf_runtime(noop(1))",
        "Value `1` of type `int` does not match the type annotation `bool` for argument `i`",
    );
    a.fail(
        r#"
def f_compile_time(i: bool):
    pass
def g():
    f_compile_time(1)
"#,
        "Expected type `bool` but got `int`",
    );
    a.pass(
        r#"Foo = record(value=int)
def f(v: bool) -> Foo:
    return Foo(value=1)"#,
    );
    // Type errors should be caught in return positions
    a.fail(
        "def f_return_runtime() -> bool:\n return noop(1)\nf_return_runtime()",
        "Value `1` of type `int` does not match the type annotation `bool` for return type",
    );
    a.fail(
        r#"
def f_return_compile_time() -> bool:
    return 1
def g():
    f_return_compile_time()
"#,
        "Expected type `bool` but got `int`",
    );
    // And for functions without return
    // TODO(nga): should be compile-time error.
    a.fail(
        "def f_bool_none() -> bool:\n pass\nf_bool_none()",
        "Value `None` of type `NoneType` does not match the type annotation `bool` for return type",
    );
    // And for functions that return None implicitly or explicitly
    a.fails(
        "def f_none_bool_runtime() -> None:\n return noop(True)\nf_none_bool_runtime()",
        &["type annotation", "`None`", "`bool`", "return"],
    );
    a.fail(
        r#"
def f_none_bool_compile_time() -> None:
    return True
def g():
    f_none_bool_compile_time()
"#,
        "Expected type `None` but got `bool`",
    );
    a.pass("def f() -> None:\n pass\nf()");

    // The following are all valid types
    a.all_true(
        r#"
isinstance(1, int)
isinstance(True, bool)
isinstance(True, typing.Any)
isinstance(None, None)
isinstance(assert_type, typing.Callable)
isinstance([], list[int])
isinstance([], list[typing.Any])
isinstance([1, 2, 3], list[int])
isinstance(None, None | int)
isinstance('test', int | str)
isinstance(('test', None), (str, None))
isinstance(('test', None), tuple[str, None])
isinstance({"test": 1, "more": 2}, dict[str, int])
isinstance({1: 1, 2: 2}, dict[int, int])

not isinstance(1, None)
not isinstance((1, 1), str)
not isinstance('test', int | bool)
not isinstance([1,2,None], list[int])
not isinstance({"test": 1, 8: 2}, dict[str, int])
not isinstance({"test": 1, "more": None}, dict[str, int])

isinstance(1, typing.Any)
isinstance([1,2,"test"], list)
"#,
    );

    // Checking types fails for invalid types
    a.fail("isinstance(None, isinstance)", "not a valid type");
    a.fail("isinstance(None, [])", "expected `type`");
    a.fail("isinstance(None, [None, int])", "expected `type`");
    a.fail("isinstance(None, {'1': '', '2': ''})", "expected `type`");

    // Should check the type of default parameters that aren't used
    a.fail(
        r#"
def foo(f: int = None):
    pass
"#,
        "`None` of type `NoneType` does not match the type annotation `int`",
    );
}

#[test]
fn test_new_syntax_without_dot_type_compile_time() {
    assert::pass(r"def f() -> int: return 17");
    assert::fail(
        r#"
def f() -> int: return 'tea'
"#,
        "Expected type `int` but got `str`",
    );
}

#[test]
fn test_new_syntax_without_dot_type_runtime() {
    assert::pass(
        r#"
def f() -> str: return noop('coke')
f()
"#,
    );
    assert::fail(
        r#"
def f() -> str: return noop(19)
f()
"#,
        "Value `19` of type `int`",
    );
}

#[test]
fn test_type_compiled_display() {
    fn t(expected: &str, ty0: &str) {
        Heap::temp(|heap| {
            let ty = assert::pass(ty0).add_to_heap(heap);
            let ty = TypeCompiled::new(ty, heap).unwrap();
            assert_eq!(expected, ty.to_string(), "for `{ty0}`");
        });
    }

    t("typing.Any", "typing.Any");
    t("list", "list");
    t("list", "list[typing.Any]");
    t("None", "None");
}

#[test]
fn test_type_compiled_starlark_api() {
    assert::eq("\"int\"", "repr(eval_type(int))");
    assert::eq("\"int | str\"", "repr(eval_type(int | str))");
    assert::is_true("eval_type(int).matches(1)");
    assert::is_true("not eval_type(int).matches([])");
    assert::pass("eval_type(int).check_matches(1)");
    assert::fail(
        "eval_type(int).check_matches([])",
        "Value of type `list` does not match type `int`: []",
    );
}

#[test]
fn test_eval_type_eval_type() {
    assert::is_true("isinstance(1, eval_type(eval_type(int)))");
}

#[test]
fn test_type_compiled_can_be_used_in_function_signature() {
    assert::pass(
        r#"
ty = eval_type(int)
def f(x: ty):
    pass

f(1)
"#,
    );
    assert::fail(
        r#"
ty = eval_type(int)
def f(x: ty):
    pass

# Runtime error.
f(noop("x"))
"#,
        "Value `x` of type `string` does not match the type annotation `int` for argument `x`",
    );
    assert::fail(
        r#"
ty = eval_type(int)
def f(x: ty):
    pass

def g():
    # Compile-time error.
    f("x")
"#,
        "Expected type `int` but got `str`",
    );
}

#[test]
fn test_isinstance() {
    assert::eq("True", "isinstance(1, int)");
    assert::eq("False", "isinstance(1, str)");
}

#[test]
fn test_new_list_dict_syntax_pass() {
    assert::pass(
        r#"
def uuu(x: list[int]):
    pass

uuu([1, 2, 3])
"#,
    );
}

#[test]
fn test_new_list_dict_syntax_fail_compile_time() {
    assert::fail(
        r#"
def uuu(x: list[int]):
    pass

def www():
    uuu(["mm"])
"#,
        "Expected type `list[int]` but got `list[str]`",
    );
}

#[test]
fn test_new_list_dict_syntax_fail_runtime() {
    assert::fail(
        r#"
def uuu(x: list[int]):
    pass

noop(uuu)(["mm"])
"#,
        r#"Value `["mm"]` of type `list` does not match"#,
    );
}

#[test]
fn test_bit_or() {
    let types = [
        ("int", "17"),
        ("str", "'x'"),
        ("None", "None"),
        ("list", "[]"),
        ("(str | int)", "19"),
    ];
    for (at, av) in &types {
        for (bt, bv) in &types {
            assert::is_true(&format!("isinstance({av}, {at} | {bt})"));
            assert::is_true(&format!("isinstance({bv}, {at} | {bt})"));
            assert::is_true(&format!("not isinstance(range(10), {at} | {bt})"));
        }
    }
}

/// `tuple[A]` and `tuple[A, B]` as in Python. The grammar has no third subscript, and
/// `tuple[(A, B, C)]` cannot stand for `tuple[A, B, C]` because a tuple of types is itself a
/// type, so a longer tuple type is spelled as that tuple of types, `(A, B, C)`.
#[test]
fn test_tuple_type_expressions() {
    let a = crate::assert::Assert::new();
    a.all_true(
        r#"
isinstance((1,), tuple[int])
isinstance((1, "a"), tuple[int, str])
isinstance((1, "a", None), (int, str, None))
isinstance(((1, "a", None),), tuple[(int, str, None)])
not isinstance((1, "a", None), tuple[(int, str, None)])
isinstance((1, 2, 3), tuple[int, ...])
not isinstance((1, "a"), tuple[int])
not isinstance((1, "a"), tuple[str, int])
not isinstance((1, "a", "b"), tuple[int, str])
not isinstance([1, "a"], tuple[int, str])
"#,
    );
    a.pass(
        r#"
def f(x: tuple[int, str]) -> tuple[str, int]:
    return (x[1], x[0])
f((1, "a"))
"#,
    );
}

/// `accepted` runs; followed by `then`, it fails with `error`.
fn passes_then_fails(a: &assert::Assert, accepted: &str, then: &str, error: &str) {
    a.pass(accepted);
    a.fail(&format!("{accepted}\n{then}\n"), error);
}

#[test]
fn test_dict_type_is_checked_again_after_a_mutation() {
    let a = assert::Assert::new();
    let rejected = "does not match the type annotation `dict[str, int]` for argument `d`";
    // `f` accepts the dict, a mutation breaks its type, and `f` must then reject it.
    let accepted = r#"
def f(d: dict[str, int]):
    pass
d = noop({"a": 1, "b": 2})
for i in range(40):
    d["k" + str(i)] = i
f(d)
f(d)
d["z"] = 26
f(d)
"#;
    for mutation in [
        r#"d["c"] = "x""#,
        r#"d["a"] = "x""#,
        r#"d[1] = 2"#,
        r#"d.update({"c": "x"})"#,
        r#"d.setdefault("c", "x")"#,
        r#"d |= {"c": "x"}"#,
        r#"d.pop("a"); d["a"] = "x""#,
        r#"d.clear(); d[1] = 2"#,
    ] {
        passes_then_fails(&a, accepted, &format!("{mutation}\nf(d)"), rejected);
    }
    // What one annotation accepted is nothing to another.
    passes_then_fails(
        &a,
        r#"
def f(d: dict[str, int]):
    pass
def g(d: dict[str, int | str]):
    pass
d = noop({"k" + str(i): i for i in range(40)})
f(d)
g(d)
d["b"] = "x"
g(d)
"#,
        "f(d)",
        rejected,
    );
    // Nor is it anything to another annotation of the same type.
    passes_then_fails(
        &a,
        r#"
def f(d: dict[str, int]):
    pass
def g(d: dict[str, int]):
    pass
d = noop({"k" + str(i): i for i in range(40)})
f(d)
g(d)
"#,
        r#"d["k0"] = "x"
g(d)"#,
        rejected,
    );
    // A value a dict holds can change without the dict changing.
    for (value_type, value, mutation) in [
        ("list[int]", "[i]", r#"d["k0"].append("x")"#),
        (
            "tuple[list[int], int]",
            "([i], 2)",
            r#"d["k0"][0].append("x")"#,
        ),
        ("list[int] | None", "[i]", r#"d["k0"].append("x")"#),
        ("list[int] | None | str", "[i]", r#"d["k0"].append("x")"#),
        (
            "tuple[list[int], ...]",
            "([i], [i])",
            r#"d["k0"][1].append("x")"#,
        ),
        ("(list[int],)", "([i],)", r#"d["k0"][0].append("x")"#),
        (
            "(list[int], int, str)",
            r#"([i], 2, "s")"#,
            r#"d["k0"][0].append("x")"#,
        ),
        // The value that can change is not the first the type names.
        ("(int, list[int])", "(2, [i])", r#"d["k0"][1].append("x")"#),
        (
            "(int, str, list[int])",
            r#"(2, "s", [i])"#,
            r#"d["k0"][2].append("x")"#,
        ),
        ("list[int] | (int, int)", "[i]", r#"d["k0"].append("x")"#),
        ("(int, int) | list[int]", "[i]", r#"d["k0"].append("x")"#),
        ("dict[str, int]", r#"{"a": i}"#, r#"d["k0"]["b"] = "x""#),
        ("set[int]", "set([i])", r#"d["k0"].add("x")"#),
    ] {
        passes_then_fails(
            &a,
            &format!(
                r#"
def f(d: dict[str, {value_type}]):
    pass
d = noop({{"k" + str(i): {value} for i in range(40)}})
f(d)
"#
            ),
            &format!("{mutation}\nf(d)"),
            "for argument `d`",
        );
    }
    a.pass(
        r#"
Rec = record(x = int)
def f(d: dict[(str, int), Rec | None]):
    pass
d = noop({})
for i in range(100):
    d[("k", i)] = Rec(x = i) if i % 2 == 0 else None
    f(d)
assert_true(isinstance(d, dict[(str, int), Rec | None]))
d[("k", 0)] = 1
assert_false(isinstance(d, dict[(str, int), Rec | None]))
"#,
    );
}

#[test]
fn test_dict_type_agrees_with_testing_every_entry() {
    // Random edits of one dict; after each, `dict[str, int]` must say what looking at every
    // entry says. `dict[str, int | str]`, which accepts every one of these dicts, takes turns
    // with it, so each finds what the other left behind.
    Heap::temp(|heap| {
        let d = heap.alloc(Dict::new(SmallMap::new())).to_value();
        let narrow = IsDictOf::new(IsStr, IsInt);
        let wide = IsDictOf::new(IsStr, IsAnyOfTwo(IsInt, IsStr));
        let mut seed = 0x2545f4914f6cdd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut rejected = 0;
        for step in 0..20000i32 {
            let key = heap.alloc(format!("k{}", next(40))).to_value();
            // One edit in twenty breaks the type.
            let value = if next(20) == 0 {
                heap.alloc("not an int").to_value()
            } else {
                heap.alloc(step).to_value()
            };
            match next(10) {
                0 => {
                    let mut dict = DictMut::from_value(d).unwrap();
                    dict.aref.remove_hashed(key.get_hashed().unwrap());
                }
                1 if next(4) == 0 => DictMut::from_value(d).unwrap().aref.clear(),
                2 => {
                    let mut dict = DictMut::from_value(d).unwrap();
                    dict.aref.insert_hashed(key.get_hashed().unwrap(), value);
                }
                _ => d.set_at(key, value).unwrap(),
            }
            let expected = DictRef::from_value(d)
                .unwrap()
                .iter()
                .all(|(k, v)| k.unpack_str().is_some() && v.unpack_i32().is_some());
            rejected += !expected as usize;
            for _ in 0..next(3) {
                if next(2) == 0 {
                    assert_eq!(expected, narrow.matches(d), "step {step}");
                } else {
                    assert!(wide.matches(d), "step {step}");
                }
            }
        }
        // Both answers are well represented.
        assert!(rejected > 2000 && rejected < 18000, "{rejected}");
    });
}
