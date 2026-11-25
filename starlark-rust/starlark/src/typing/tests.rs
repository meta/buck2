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

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Write;

use allocative::Allocative;
use dupe::Dupe;
use starlark_derive::NoSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;
use starlark_syntax::golden_test_template::golden_test_template;

use crate as starlark;
use crate::assert::Assert;
use crate::environment::FrozenModule;
use crate::environment::GlobalsBuilder;
use crate::environment::Module;
use crate::eval::Evaluator;
use crate::eval::runtime::file_loader::ReturnOwnedFileLoader;
use crate::syntax::AstModule;
use crate::syntax::Dialect;
use crate::tests::util::trim_rust_backtrace;
use crate::typing::AstModuleTypecheck;
use crate::typing::OperatorSignature;
use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::TypingBinOp;
use crate::typing::TypingUnOp;
use crate::typing::callable_param::ParamIsRequired;
use crate::typing::interface::Interface;
use crate::util::ArcStr;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::StarlarkDict;
use crate::values::layout::heap::name::StarlarkTestHeapName;
use crate::values::none::NoneType;
use crate::values::string::StarlarkStr;
use crate::values::types::int::view::IntView;
use crate::values::typing::StarlarkCallable;
use crate::values::typing::StarlarkCallableParamSpec;
use crate::values::typing::StarlarkIter;

mod call;
mod callable;
mod calls_and_bindings;
mod list;
mod special_function;
mod tuple;
mod types;

#[derive(Default)]
struct TypeCheck {
    expect_types: Vec<String>,
    loads: HashMap<String, (Interface, FrozenModule)>,
}

/// A simple custom type for testing `StarlarkValueAsType` parameterization errors.
#[derive(
    derive_more::Display,
    Debug,
    NoSerialize,
    Allocative,
    ProvidesStaticType,
    StarlarkPagable
)]
#[display("MyType")]
struct MyCustomType;

#[starlark_value(type = "my_custom_type")]
impl<'v> StarlarkValue<'v> for MyCustomType {}

/// A type with every operator, each written with the signature the trait declares.
#[derive(
    derive_more::Display,
    Debug,
    NoSerialize,
    Allocative,
    ProvidesStaticType,
    StarlarkPagable
)]
#[display("EveryOperator")]
struct EveryOperator;

#[starlark_value(type = "every_operator")]
impl<'v> StarlarkValue<'v> for EveryOperator {
    fn add(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        Some(Ok(other))
    }

    fn radd(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        Some(Ok(other))
    }

    fn mul(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        Some(Ok(other))
    }

    fn rmul(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        Some(Ok(other))
    }

    fn sub(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn div(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn percent(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn floor_div(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn bit_and(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn bit_or(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn bit_xor(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn left_shift(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn right_shift(_this: &Self, other: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(other)
    }

    fn plus(_this: &Self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(Value::new_none())
    }

    fn minus(_this: &Self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(Value::new_none())
    }

    fn bit_not(_this: &Self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(Value::new_none())
    }

    fn compare(_this: &Self, _other: Value<'v>) -> crate::Result<Ordering> {
        Ok(Ordering::Equal)
    }

    fn is_in(_this: &Self, _other: Value<'v>) -> crate::Result<bool> {
        Ok(true)
    }

    fn at(_this: &Self, index: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(index)
    }
}

/// A type that writes the signatures of its operators by hand: `+` with a result that
/// depends on the operand, a reflected `*`, unary `-`, and indexing.
#[derive(
    derive_more::Display,
    Debug,
    NoSerialize,
    Allocative,
    ProvidesStaticType,
    StarlarkPagable
)]
#[display("WithSignatures")]
struct WithSignatures;

#[starlark_value(type = "with_signatures")]
impl<'v> StarlarkValue<'v> for WithSignatures {
    fn bin_op_signatures(op: TypingBinOp) -> Vec<OperatorSignature> {
        match op {
            TypingBinOp::Add => vec![
                OperatorSignature {
                    operand: Ty::int(),
                    result: Ty::string(),
                },
                OperatorSignature {
                    operand: Ty::float(),
                    result: Ty::bool(),
                },
            ],
            _ => Vec::new(),
        }
    }

    fn rbin_op_signatures(op: TypingBinOp) -> Vec<OperatorSignature> {
        match op {
            TypingBinOp::Mul => vec![OperatorSignature {
                operand: Ty::float(),
                result: Ty::string(),
            }],
            _ => Vec::new(),
        }
    }

    fn un_op_result(op: TypingUnOp) -> Option<Ty> {
        match op {
            TypingUnOp::Minus => Some(Ty::int()),
            _ => None,
        }
    }

    fn at(_this: &Self, index: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(index)
    }

    fn at_signature() -> Option<OperatorSignature> {
        Some(OperatorSignature {
            operand: Ty::int(),
            result: Ty::string(),
        })
    }
}

struct NamedXy;

pagable::static_str!(NAMED_XY_X = "x");
pagable::static_str!(NAMED_XY_Y = "y");

impl StarlarkCallableParamSpec for NamedXy {
    fn params() -> ParamSpec {
        ParamSpec::new_named_only([
            (
                ArcStr::new_static(NAMED_XY_X),
                ParamIsRequired::Yes,
                Ty::string(),
            ),
            (
                ArcStr::new_static(NAMED_XY_Y),
                ParamIsRequired::Yes,
                Ty::int(),
            ),
        ])
        .unwrap()
    }
}

#[starlark_module]
#[starlark_types(
    MyCustomType as MyCustomType,
    EveryOperator as EveryOperator,
    WithSignatures as WithSignatures
)]
fn register_typecheck_globals(globals: &mut GlobalsBuilder) {
    fn accepts_iterable<'v>(
        #[starlark(require = pos)] xs: ValueOf<'v, StarlarkIter<Value<'v>>>,
    ) -> anyhow::Result<NoneType> {
        let _ignore = xs;
        Ok(NoneType)
    }

    fn accepts_typed_kwargs<'v>(
        #[starlark(kwargs)] x: ValueOf<'v, StarlarkDict<'v, StarlarkStr, IntView<'v>>>,
    ) -> anyhow::Result<NoneType> {
        let _ignore = x;
        Ok(NoneType)
    }

    fn accepts_callable_named_xy<'v>(
        #[starlark(require = pos)] f: StarlarkCallable<'v, NamedXy, NoneType>,
    ) -> anyhow::Result<NoneType> {
        let _ignore = f;
        Ok(NoneType)
    }
}

impl TypeCheck {
    fn new() -> Self {
        Self::default()
    }

    fn ty(mut self, name: &str) -> Self {
        self.expect_types.push(name.to_owned());
        self
    }

    fn load(mut self, file: &str, interface: Interface, module: FrozenModule) -> Self {
        self.loads.insert(file.to_owned(), (interface, module));
        self
    }

    fn mk_file_loader(&self) -> ReturnOwnedFileLoader {
        let modules = self
            .loads
            .iter()
            .map(|(name, (_, module))| (name.clone(), module.dupe()))
            .collect();
        ReturnOwnedFileLoader { modules }
    }

    fn check(&self, test_name: &str, code: &str) -> (Interface, FrozenModule) {
        let globals = GlobalsBuilder::extended()
            .with(register_typecheck_globals)
            .build();
        let ast =
            AstModule::parse("filename", code.to_owned(), &Dialect::AllOptionsInternal).unwrap();
        let (errors, typemap, interface, approximations) = ast.clone().typecheck(
            &globals,
            &self
                .loads
                .iter()
                .map(|(name, (intf, _))| (name.clone(), intf.clone()))
                .collect(),
        );

        let mut output = String::new();
        writeln!(output, "Code:").unwrap();
        writeln!(output, "{}", code.trim()).unwrap();
        if errors.is_empty() {
            writeln!(output).unwrap();
            writeln!(output, "No errors.").unwrap();
        } else {
            for error in &errors {
                writeln!(output).unwrap();
                writeln!(output, "Error:").unwrap();
                // Note we are using `:#` here instead of `:?` because
                // `:?` includes rust backtrace.
                // The issue: https://github.com/dtolnay/anyhow/issues/300
                writeln!(output, "{}", format!("{error:#}").trim_end()).unwrap();
            }
        }

        if !approximations.is_empty() {
            writeln!(output).unwrap();
            writeln!(output, "Approximations:").unwrap();
            for appox in approximations {
                writeln!(output, "{appox}").unwrap();
            }
        }

        if !self.expect_types.is_empty() {
            writeln!(output).unwrap();
            writeln!(output, "Types:").unwrap();
            for k in &self.expect_types {
                let types = typemap.find_bindings_by_name(k);
                match types.as_slice() {
                    [ty] => writeln!(output, "{k}: {ty}").unwrap(),
                    [] => panic!("Type not found for {k}"),
                    [_, _, ..] => panic!("Multiple types found for {k}"),
                }
            }
        }

        let loader = self.mk_file_loader();
        let module = {
            writeln!(output).unwrap();
            writeln!(output, "Compiler typechecker (eval):").unwrap();
            Module::with_temp_heap(|module| {
                let mut eval = Evaluator::new(&module);

                eval.set_loader(&loader);

                eval.enable_static_typechecking(true);
                let eval_result = eval.eval_module(ast, &globals);
                if eval_result.is_ok() != errors.is_empty() {
                    writeln!(output, "Compiler typechecker and eval results mismatch.").unwrap();
                    writeln!(output).unwrap();
                }

                // Additional writes must happen above this line otherwise it might be erased by trim_rust_backtrace
                match &eval_result {
                    Ok(_) => writeln!(output, "No errors.").unwrap(),
                    Err(err) => writeln!(output, "{err:?}").unwrap(),
                }

                // Help borrow checker.
                drop(eval);

                module.freeze_named(StarlarkTestHeapName::frozen_heap_name())
            })
            .unwrap()
        };

        golden_test_template(
            &format!("src/typing/tests/golden/{test_name}.golden"),
            trim_rust_backtrace(&output),
        );

        (interface, module)
    }
}

#[test]
fn test_success() {
    TypeCheck::default().ty("y").check(
        "success",
        r#"
def foo(x: str) -> str:
    return x.removeprefix("test")

def bar():
    y = hash(foo("magic"))
"#,
    );
}

#[test]
fn test_failure() {
    TypeCheck::new().check(
        "failure",
        r#"
def test():
    hash(1)
"#,
    );
}

#[test]
fn test_load() {
    let (interface, module) = TypeCheck::new().check(
        "load_0",
        r#"
def foo(x: list[bool]) -> str:
    return "test"
   "#,
    );
    TypeCheck::new()
        .load("foo.bzl", interface, module)
        .ty("res")
        .check(
            "load_1",
            r#"
load("foo.bzl", "foo")
def test():
    res = [foo([])]
"#,
        );
}

/// Test things that have previous claimed incorrectly they were type errors
#[test]
fn test_false_negative() {
    TypeCheck::new().check(
        "false_negative",
        r#"
def test():
    fail("Expected variable expansion in string: `{}`".format("x"))
"#,
    );
}

#[test]
fn test_dot_type() {
    TypeCheck::new().check(
        "dot_type_0",
        r#"
def foo(x: list) -> bool:
    return type(x) == type(list)

def bar():
    foo([1,2,3])
"#,
    );
    TypeCheck::new().check(
        "dot_type_1",
        r#"
def foo(x: list) -> bool:
    return type(x) == []

def bar():
    foo(True)
"#,
    );
}

#[test]
fn test_accepts_iterable() {
    TypeCheck::new().check(
        "accepts_iterable",
        r#"
def test():
    accepts_iterable([1, ()])
"#,
    );

    let mut a = Assert::new();
    a.globals_add(register_typecheck_globals);
    a.pass("accepts_iterable([1, ()])");
}

#[test]
fn test_dict_bug() {
    // TODO(nga): figure out how to fix it.
    //   Type of `y` should be inferred to `str`.
    TypeCheck::new().ty("y").check(
        "dict_bug",
        r#"
def test():
    x = {}
    x.setdefault(33, "x")
    y = x[44]
"#,
    );
}

#[test]
fn test_dict_lookup_by_never() {
    TypeCheck::new().check(
        "dict_never_key",
        r#"
# We use `typing.Never` when expression is an error,
# or it is a type parameter of empty list for example.
# Dict lookup by never should not be an error.
def test(d: dict[typing.Any, str], x: typing.Never):
    y = d[x]
"#,
    );
}

#[test]
fn test_new_list_dict_syntax() {
    TypeCheck::new().ty("x").check(
        "new_list_dict_syntax",
        r#"
def new_list_dict_syntax(d: dict[str, int]) -> list[str]:
    return list(d.keys())

def test():
    # Check type is properly parsed from the function return type.
    x = new_list_dict_syntax({"a": 1, "b": 2})
"#,
    );
}

#[test]
fn test_new_list_dict_syntax_as_value() {
    // TODO(nga): fix.
    TypeCheck::new().ty("x").ty("y").check(
        "new_list_dict_syntax_as_value",
        r#"
def test():
    x = list[str]
    y = dict[int, bool]
"#,
    );
}

#[test]
fn test_int_plus_float() {
    TypeCheck::new().ty("x").check(
        "int_plus_float",
        r#"
def test():
    x = 1 + 1.0
"#,
    );
}

#[test]
fn test_int_bitor_float() {
    TypeCheck::new().ty("x").check(
        "int_bitor_float",
        r#"
def test():
    x = 0x60000000000000000000000 | 1.0
"#,
    );
}

#[test]
fn test_un_op() {
    TypeCheck::new().ty("x").ty("y").ty("z").check(
        "un_op",
        r#"
def test():
    # Good.
    x = -1
    # Bad.
    y = ~True
    # Union good and bad.
    z = -(1 if True else "")
"#,
    );
}

#[test]
fn test_operators_without_types() {
    TypeCheck::new()
        .ty("binary")
        .ty("boolean")
        .ty("reflected")
        .ty("unary")
        .check(
            "operators_without_types",
            r#"
def test(x: EveryOperator, y: str):
    binary = (x + y, x - y, x * y, x / y, x // y, x % y, x & y, x | y, x ^ y, x << y, x >> y)
    boolean = (x < y, y in x)
    reflected = (None + x, None * x)
    unary = (+x, -x, ~x)
"#,
        );
}

/// Indexing a type that says what index it takes and what is there, one that has `at` and
/// says nothing, and one that has no `at`.
#[test]
fn test_index_signature() {
    TypeCheck::new()
        .ty("by_int")
        .ty("by_unknown")
        .ty("by_str")
        .ty("unsaid")
        .ty("not_indexed")
        .check(
            "index_signature",
            r#"
def test(x: WithSignatures, y: EveryOperator, z: MyCustomType, i: int, s: str, a):
    by_int = x[i]
    by_unknown = x[a]
    by_str = x[s]
    unsaid = y[s]
    not_indexed = z[i]
"#,
        );
}

#[test]
fn test_operator_signatures() {
    TypeCheck::new()
        .ty("one_int")
        .ty("one_float")
        .ty("both")
        .ty("unknown")
        .ty("none")
        .ty("reflected")
        .ty("reflected_none")
        .ty("unary")
        .ty("unary_none")
        .check(
            "operator_signatures",
            r#"
def test(x: WithSignatures, i: int, f: float, n: int | float, a, s: str):
    # The operand matches one signature.
    one_int = x + i
    one_float = x + f
    # The operand matches both.
    both = x + n
    unknown = x + a
    # The operand matches none.
    none = x + s
    # The reflected operator, for a left operand whose type does not have the operator.
    reflected = f * x
    reflected_none = None * x
    # The unary operators.
    unary = -x
    unary_none = ~x
"#,
        );
}

/// `<` takes what the type's `compare` takes.
#[test]
fn test_compare_operand() {
    TypeCheck::new().ty("good").ty("bad").check(
        "compare_operand",
        r#"
def test(s: str, i: int):
    good = s < "x"
    bad = s < i
"#,
    );
}

/// The operators of `str`, `bytes` and `tuple` take and give what their bodies say, and the
/// typechecker's own rules for `list`, `dict` and `set` answer before the bodies are asked.
#[test]
fn test_operators_of_builtin_types() {
    TypeCheck::new()
        .ty("str_add")
        .ty("str_mul")
        .ty("int_mul_str")
        .ty("str_percent")
        .ty("bytes_add")
        .ty("bytes_mul")
        .ty("tuple_add")
        .ty("tuple_mul")
        .ty("list_add")
        .ty("list_mul")
        .ty("dict_or")
        .ty("set_or")
        .check(
            "operators_of_builtin_types",
            r#"
def test(s: str, b: bytes, t: tuple[int, ...], i: int, a):
    str_add = s + s
    str_mul = s * i
    int_mul_str = i * s
    str_percent = s % a
    bytes_add = b + b
    bytes_mul = b * i
    tuple_add = t + t
    tuple_mul = t * i
    list_add = [i] + [s]
    list_mul = [i] * i
    dict_or = {i: s} | {s: i}
    set_or = set([i]) | set([s])

def bad(s: str, b: bytes, t: tuple[int, ...], i: int):
    s + i
    s * s
    b + s
    t + [i]
    t * s
"#,
        );
}

/// Indexing and `in` on `str`, `bytes` and `range` take and give what their bodies say, and
/// the typechecker's own rules for `list`, `tuple`, `dict` and `set` answer before the bodies
/// are asked.
#[test]
fn test_index_and_in_of_builtin_types() {
    TypeCheck::new()
        .ty("str_at")
        .ty("str_in")
        .ty("bytes_at")
        .ty("bytes_in")
        .ty("range_at")
        .ty("range_in")
        .ty("list_at")
        .ty("list_in")
        .ty("tuple_at")
        .ty("tuple_in")
        .ty("dict_at")
        .ty("dict_in")
        .ty("set_in")
        .check(
            "index_and_in_of_builtin_types",
            r#"
def test(s: str, b: bytes, r: range, t: tuple[int, ...], i: int, a):
    str_at = s[i]
    str_in = s in s
    bytes_at = b[i]
    bytes_in = i in b
    range_at = r[i]
    range_in = s in r
    list_at = [s][i]
    list_in = s in [s]
    tuple_at = t[i]
    tuple_in = s in t
    dict_at = {s: i}[s]
    dict_in = s in {s: i}
    set_in = s in set([s])
    s[a]
    a in s

def bad(s: str, b: bytes, r: range, t: tuple[int, ...], i: int):
    s[s]
    i in s
    b[s]
    r[s]
    [i][s]
    t[s]
    {s: i}[i]
    i in [s]
    i in {s: i}
"#,
        );
}

/// The operators of `int` and `float`, whose result depends on the operand.
#[test]
fn test_operators_of_numbers() {
    TypeCheck::new()
        .ty("int_add_int")
        .ty("int_add_float")
        .ty("int_add_any")
        .ty("float_add_int")
        .ty("float_add_any")
        .ty("int_div_int")
        .ty("int_floor_div_float")
        .ty("int_percent_int")
        .ty("int_or_int")
        .ty("int_or_any")
        .ty("int_shift_int")
        .ty("int_less_float")
        .ty("float_less_any")
        .check(
            "operators_of_numbers",
            r#"
def test(i: int, f: float, a):
    int_add_int = i + i
    int_add_float = i + f
    int_add_any = i + a
    float_add_int = f + i
    float_add_any = f * a
    int_div_int = i / i
    int_floor_div_float = i // f
    int_percent_int = i % i
    int_or_int = i | i
    int_or_any = i ^ a
    int_shift_int = i << i
    int_less_float = i < f
    float_less_any = f < a

def bad(i: int, f: float, s: str):
    i + s
    f - s
    i / s
    i | f
    f & f
    i << f
    i < s
    f < s
    i in i
"#,
        );
}

#[test]
fn test_union() {
    TypeCheck::new().check(
        "union",
        r#"
def func_which_returns_union(p) -> str | int:
    if p == 56:
        return "a"
    elif p == 57:
        return 1
    else:
        return []
"#,
    );
}

#[test]
fn test_methods_work_for_ty_starlark_value() {
    TypeCheck::new().ty("x").check(
        "methods_work_for_ty_starlark_value",
        r#"
def test(s: str):
    x = s.startswith("a")
"#,
    );
}

#[test]
fn test_bit_or_return_int() {
    TypeCheck::new().check(
        "bit_or_return_int",
        r#"
test = int | 3

def foo() -> test:
    pass
"#,
    );
}

#[test]
fn test_bit_or_return_list() {
    TypeCheck::new().check(
        "bit_or_return_list",
        r#"
test = int | list[3]

def foo() -> test:
    pass
"#,
    );
}

#[test]
fn test_bit_or_with_load() {
    let (interface, module) = TypeCheck::new().check(
        "test_bit_or_with_load_foo",
        r#"
def foo() -> str:
    return "test"
"#,
    );
    TypeCheck::new().load("foo.bzl", interface, module).check(
        "test_bit_or_with_load",
        r#"
load("foo.bzl", "foo")
test = int | foo()
def test() -> test:
    pass
"#,
    );
}
