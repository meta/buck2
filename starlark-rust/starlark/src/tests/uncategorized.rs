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

use std::cell::RefCell;
use std::fmt::Write;

use allocative::Allocative;
use anyhow::Context;
use derive_more::Display;
use either::Either;
use starlark_derive::StarlarkPagable;
use starlark_derive::StarlarkRepresentations;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;
use starlark_syntax::golden_test_template::golden_test_template;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::assert;
use crate::assert::Assert;
use crate::collections::SmallMap;
use crate::environment::Globals;
use crate::environment::GlobalsBuilder;
use crate::environment::Module;
use crate::eval::Evaluator;
use crate::starlark_simple_value;
use crate::syntax::AstModule;
use crate::syntax::Dialect;
use crate::tests::util::trim_rust_backtrace;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::Heap;
use crate::values::NoSerialize;
use crate::values::StarlarkType;
use crate::values::StarlarkValue;
use crate::values::Trace;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::dict::DictType;
use crate::values::layout::heap::name::StarlarkTestHeapName;
use crate::values::list_or_tuple::ListOrTupleType;
use crate::values::none::NoneType;
use crate::values::string::StarlarkStr;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::types::int::view::IntView;

#[test]
fn alias_test() {
    assert::is_true(
        r#"
a = [1, 2, 3]
b = a
a[2] = 0
a == [1, 2, 0] and b == [1, 2, 0]
"#,
    )
}

#[test]
fn test_bad_break() {
    assert::fails("break", &["break", "outside of", "loop"]);
    assert::fails(
        "def foo(x):\n  if 1:\n    break",
        &["break", "outside of", "loop"],
    );
    assert::fails(
        "def foo(x):\n  if 1:\n    continue",
        &["continue", "outside of", "loop"],
    );
    assert::fail(
        "
def foo(x):
    for y in x:
        def bar(y):
            continue",
        "outside of",
    );
    assert::fail("return 1", "outside of a `def`");
    assert::fail("for x in []:\n  return 1", "outside of a `def`");
}

#[test]
fn test_tabs_fail() {
    let a = Assert::new();
    a.fail("def f():\n\tpass", "Parse error");
    a.fail("def f():\n x\t=3", "Parse error");
}

#[test]
fn test_top_level_statements() {
    assert::pass(
        r#"
j = 0
for i in range(10):
    if i % 2 == 0:
        j += i
assert_eq(j, 20)
"#,
    );
}

#[test]
fn test_compiled_literals() {
    assert::is_true(
        "
def f():
    return [[]]
y = f()
y.append(1)
y == [[],1]",
    );
    assert::is_true(
        "
def f():
    return {1:2}
y = f()
y[3] = 4
y == {1:2, 3:4}",
    );
    // This test breaks if we compile constants deep compile the literals
    // and don't deep thaw them
    assert::is_true(
        "
def f():
    return [[]]
y = f()[0]
y.append(1)
y == [1]",
    );
}

#[test]
fn test_frozen_iteration() {
    // nested iteration
    assert::is_true(
        r#"
def loop():
    xs = [1, 2, 3]
    z = 0
    for x in xs:
        for y in xs:
            z += x + y
    return z
loop() == 36"#,
    );
    // iterate, mutate, iterate
    assert::is_true(
        r#"
def loop():
    y = 0
    xs = [1, 2, 3]
    for x in xs:
        y += x
    xs.append(4)
    for x in xs:
        y += x
    return y
loop() == 16"#,
    );
    // iterate and mutate at the same time
    assert::fail(
        r#"
def loop():
    xs = [1, 2, 3]
    for x in xs:
        if len(xs) == 3:
            xs.append(4)
loop()"#,
        "mutates an iterable",
    );
}

#[test]
fn test_lvalue_once() {
    assert::is_true(
        r#"
ys = [1]
xs = [1,2,3,4]

def f():
    return ys[0]

def g():
    ys[0] = 2;
    return 10

xs[f()] += g()
# f must be evaluated first, and only once
xs == [1,12,3,4]
"#,
    );
    assert::is_true(
        r#"
ys = [1]
xs = [1,2,3,4]

def f():
    return ys[0]

def g():
    ys[0] = 2;
    return 10

xs[f()] = g()
xs == [1, 2, 10, 4]
"#,
    );
}

#[test]
fn test_add_assign() {
    // += behaves differently on different types
    assert::pass(
        r#"
x = 1
x += 8
assert_eq(x, 9)"#,
    );
    assert::pass(
        r#"
orig = [1, 2]
x = orig
x += [3]
assert_eq(x, [1, 2, 3])
assert_eq(orig, [1, 2, 3])
"#,
    );
    assert::pass(
        r#"
orig = (1, 2)
x = orig
# TODO(nga): typechecker should accept it.
x = noop(x)
x += (3,)
assert_eq(x, (1, 2, 3))
assert_eq(orig, (1, 2))
"#,
    );
    assert::fail(
        r#"
x = {1: 2}
x += {3: 4}
"#,
        "not supported",
    );
    assert::pass(
        r#"
x = [1, 2]
x[0] += 5
assert_eq(x, [6, 2])
"#,
    );
    assert::pass(
        r#"
x = {1: 2}
x[1] += 5
assert_eq(x, {1: 7})
"#,
    );
    assert::fail(
        r#"
def foo():
    xs = [1, 2]
    for x in xs:
        xs += [1]
        break
foo()
"#,
        "mutates an iterable",
    );
    assert::fail(
        r#"
xs = (1, 2)
xs[1] += 1
"#,
        "Immutable",
    );
}

#[test]
fn test_radd() {
    // We want select append to always produce a select, much like the
    // Bazel/Buck `select` function.
    #[derive(
        Debug,
        Display,
        Clone,
        ProvidesStaticType,
        NoSerialize,
        Allocative,
        StarlarkPagable
    )]
    #[display("${:?}", _0)]
    struct Select(Vec<i32>);
    starlark_simple_value!(Select);

    impl<'v> UnpackValue<'v> for Select {
        type Type = Either<Select, ListOrTupleType<IntView<'v>>>;
        type Error = crate::Error;

        fn unpack(
            value: ValueOf<'v, Self::Type>,
            get: <Self::Type as StarlarkTypeExpr<'v>>::Get,
        ) -> crate::Result<Self> {
            match get {
                Either::Left(x) => Ok(x.as_ref().clone()),
                Either::Right(_) => Ok(Select(
                    ValueOf::<ListOrTupleType<IntView>>::new_unchecked(value.to_value())
                        .try_items::<IntView>()
                        .map(|x| x?.to_integer())
                        .collect::<crate::Result<_>>()?,
                )),
            }
        }
    }

    impl Select {
        fn add(mut self, x: &Select) -> Self {
            self.0.extend(x.0.iter().copied());
            self
        }
    }

    #[starlark_value(type = "select")]
    impl<'v> StarlarkValue<'v> for Select {
        fn radd(&self, lhs: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
            let lhs: Select = Select::unpack_value(lhs).unwrap().unwrap();
            Some(Ok(heap.alloc_representation(lhs.add(self)).to_value()))
        }
        fn add(&self, rhs: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
            let rhs: Select = UnpackValue::unpack_value(rhs).unwrap().unwrap();
            Some(Ok(heap
                .alloc_representation(self.clone().add(&rhs))
                .to_value()))
        }
        fn collect_repr(&self, collector: &mut String) {
            write!(collector, "{self}").unwrap()
        }
    }

    #[starlark_module]
    fn module(build: &mut GlobalsBuilder) {
        fn select<'v>(
            xs: ValueOf<'v, ListOrTupleType<IntView<'v>>>,
            heap: Heap<'v>,
        ) -> anyhow::Result<ValueOf<'v, Select>> {
            let select = Select(
                xs.try_items::<i32>()
                    .collect::<crate::Result<_>>()
                    .map_err(|e| e.into_anyhow())?,
            );
            Ok(ValueOf::alloc(heap, select))
        }
    }

    let mut a = Assert::new();
    a.globals_add(module);
    a.pass(
        r#"
s1 = select([1])
s2 = select([2])
assert_eq(repr(s1), "$[1]")
assert_eq(repr(s1 + [3]), "$[1, 3]")
assert_eq(repr([3] + s1), "$[3, 1]")
assert_eq(repr(s1 + s2), "$[1, 2]")

s1 += [3]
v = [4]
v += s2
s2 += s1
assert_eq(repr(s1), "$[1, 3]")
assert_eq(repr(v), "$[4, 2]")
assert_eq(repr(s2), "$[2, 1, 3]")
"#,
    );
}

#[test]
fn test_compound_assignment() {
    assert::pass(
        r#"
x = 1
x <<= 8
assert_eq(x, 256)"#,
    );
    assert::pass(
        r#"
x = 1
x ^= 8
assert_eq(x, 9)"#,
    );
}

#[test]
fn test_static_name_checks() {
    let a = Assert::new();
    a.fail(
        r#"
def f():
    no_name()
True"#,
        "no_name",
    );
}

#[test]
fn test_function_to_name() {
    let mut a = Assert::new();
    a.module(
        "x",
        r#"
def mine():
    pass
names = {repr: "repr", str: "str", mine: "mine"}
assert_eq(names[repr], "repr")
assert_eq(names[mine], "mine")
assert_eq(names[str], "str")
"#,
    );
    a.pass(
        r#"
load("x", "mine", "names")
assert_eq(names[repr], "repr")
assert_eq(names[mine], "mine")
assert_eq(names[str], "str")
"#,
    );
}

#[test]
// Tests diagnostics error display.
fn test_diagnostics_display() {
    fn fail1() -> anyhow::Result<()> {
        Err(anyhow::anyhow!("fail 1"))
    }

    fn fail2() -> anyhow::Result<()> {
        fail1().context("fail 2")
    }

    fn fail3() -> anyhow::Result<()> {
        fail2().context("fail 3")
    }

    #[starlark_module]
    fn module(builder: &mut GlobalsBuilder) {
        fn rust_failure() -> anyhow::Result<NoneType> {
            fail3().context("rust failure")?;
            Ok(NoneType)
        }
    }

    let mut a = Assert::new();
    a.globals_add(module);

    a.module(
        "imported",
        r#"
# blank lines to make line numbers bigger and more obvious
#
#
#
#
x = []
def should_fail():
    rust_failure()"#,
    );

    let err = a.fail(
        r#"
load('imported', 'should_fail')
should_fail()"#,
        "rust failure",
    );

    golden_test_template(
        "src/tests/uncategorized_diagnostics_display_default.golden",
        trim_rust_backtrace(&format!("{err}")),
    );

    golden_test_template(
        "src/tests/uncategorized_diagnostics_display_hash.golden",
        trim_rust_backtrace(&format!("{err:#}")),
    );

    golden_test_template(
        "src/tests/uncategorized_diagnostics_display_debug.golden",
        trim_rust_backtrace(&format!("{err:?}")),
    );
}

#[test]
// Check that errors print out "nicely"
fn test_error_display() {
    let mut a = Assert::new();
    a.module(
        "imported",
        r#"
# blank lines to make line numbers bigger and more obvious
#
#
#
#
x = []
def add2(z):
  add(z)
def add(z):
  x.append(z)"#,
    );

    let err = a.fail(
        r#"
load('imported', 'add2')
def add3(z):
    add2(z)
add3(8)"#,
        "Immutable",
    );

    golden_test_template(
        "src/tests/uncategorized_error_display.golden",
        trim_rust_backtrace(&format!("{err}")),
    );

    golden_test_template(
        "src/tests/uncategorized_error_display_hash.golden",
        trim_rust_backtrace(&format!("{err:#}")),
    );
}

#[test]
fn test_load_reexport() {
    let mut a = Assert::new();
    a.dialect_set(|d| d.enable_load_reexport = true);
    a.module("a", "x = 1");
    a.module("b", "load('a', 'x')");
    a.pass("load('b', 'x')\nassert_eq(x, 1)");

    let mut a = Assert::new();
    a.dialect_set(|d| d.enable_load_reexport = false);
    a.module("a", "x = 1");
    a.module("b", "load('a', 'x')");
    a.fail(
        "load('b', 'x')\nassert_eq(x, 1)",
        "Module symbol `x` is not exported",
    );
}

#[test]
fn test_module_visibility_preserved_by_evaluator() -> crate::Result<()> {
    // Make sure that when we use a module in the evaluator, the entering / exiting the
    // module with ScopeData preserves the visibility of symbols.

    let globals = Globals::standard();

    Module::with_temp_heap(|import| {
        import.set("a", Value::testing_new_int(1));
        import.set_private(
            import.frozen_heap(|fh, edge| edge.rebrand(fh.alloc_str("b"))),
            Value::testing_new_int(2),
        );

        {
            let mut eval = Evaluator::new(&import);
            let ast =
                AstModule::parse("prelude.bzl", "c = 3".to_owned(), &Dialect::Standard).unwrap();
            // This mutates the original module named `import`
            let _: Value = eval.eval_module(ast, &globals)?;
        }
        let frozen_import = import.freeze_named(StarlarkTestHeapName::frozen_heap_name())?;

        Module::with_temp_heap(|m_uses_public| {
            m_uses_public.import_public_symbols(&frozen_import);
            {
                let mut eval = Evaluator::new(&m_uses_public);
                let ast =
                    AstModule::parse("code.bzl", "d = a".to_owned(), &Dialect::Standard).unwrap();
                let _: Value = eval.eval_module(ast, &globals)?;
            }
            crate::Result::Ok(())
        })?;

        Module::with_temp_heap(|m_uses_private| {
            m_uses_private.import_public_symbols(&frozen_import);
            {
                let mut eval = Evaluator::new(&m_uses_private);
                let ast =
                    AstModule::parse("code.bzl", "d = b".to_owned(), &Dialect::Standard).unwrap();
                let err = eval
                    .eval_module(ast, &globals)
                    .expect_err("Evaluation should have failed using a private symbol");

                let msg = err.to_string();
                let expected_msg = "Variable `b` not found";
                assert!(
                    msg.contains(expected_msg),
                    "Expected `{expected_msg}` to be in error message `{msg}`"
                );
            }
            Ok(())
        })
    })
}

#[test]
fn test_cancellation() -> crate::Result<()> {
    // Make sure that when we use a module in the evaluator, the entering / exiting the
    // module with ScopeData preserves the visibility of symbols.

    let globals = Globals::standard();
    Module::with_temp_heap(|import| {
        let mut eval = Evaluator::new(&import);
        eval.set_check_cancelled(Box::new(|| true));

        let ast = AstModule::parse(
            "prelude.bzl",
            // Note that the exact range here is unimportant, so long as it's small enough to not trigger the "infrequent" checks
            "def loop():\n    for i in range(10):\n       pass\nloop()".to_owned(),
            &Dialect::Standard,
        )
        .unwrap();
        eval.eval_module(ast, &globals).unwrap_err();

        let ast = AstModule::parse(
            "prelude.bzl",
            // Note that the exact range here is unimportant, so long as it's large enough to trigger the "infrequent" checks
            "def loop():\n    for i in range(1000000):\n       pass\nloop()".to_owned(),
            &Dialect::Standard,
        )
        .unwrap();
        let err = eval.eval_module(ast, &globals);

        let expected = "Evaluation cancelled";
        let err_msg = format!("{err:#?}");
        if !err_msg.contains(expected) {
            panic!("Error:\n{err:#?}\nExpected:\n{expected:?}")
        }

        Ok(())
    })
}

#[test]
fn test_load_did_you_mean() {
    let mut a = Assert::new();
    a.module("categories", "colour = 1");
    a.fail(
        "load('categories', 'color')",
        "Module has no symbol `color`, did you mean `colour`?",
    );
}

#[test]
fn test_getattr_did_you_mean_builtin() {
    assert::fail(
        "[].appen",
        "Object of type `list` has no attribute `appen`, did you mean `append`?",
    );
}

#[test]
fn test_getattr_did_you_mean_custom() {
    assert::fail(
        "noop(struct(grey=1)).gray",
        "Object of type `struct` has no attribute `gray`, did you mean `grey`?",
    );
    assert::fail(
        "Rec = record(grey=int); Rec(grey=1).gray",
        "Object of type `record` has no attribute `gray`, did you mean `grey`?",
    );
}

#[test]
fn test_globals_did_you_mean() {
    assert::fail("true", "Variable `true` not found, did you mean `True`?");
}

#[test]
fn test_module_level_did_you_mean() {
    assert::fail(
        "_x = 1; print(x)",
        "Variable `x` not found, did you mean `_x`?",
    );
}

#[test]
fn test_module_level_from_def_did_you_mean() {
    assert::fail(
        "def _func(): return func",
        "Variable `func` not found, did you mean `_func`?",
    );
}

#[test]
fn test_local_from_def_did_you_mean() {
    assert::fail(
        "def f(discreet): return discrete",
        "Variable `discrete` not found, did you mean `discreet`?",
    );
}

#[test]
fn test_compr_did_you_mean() {
    assert::fail(
        "[val for value in []]",
        "Variable `val` not found, did you mean `value`?",
    );
}

#[test]
fn test_unassigned() {
    assert::fails("y = x; x = 1", &["referenced before assignment", "`x`"]);
    assert::fails(
        "def f():\n y = x; x = 1\nf()",
        &["referenced before assignment", "`x`"],
    );
    assert::fails(
        "
def f():
    y = x
    x = 1
def g(q = 1):
    f()
g()",
        &["referenced before assignment", "`x`"],
    );
    assert::fails_skip_typecheck(
        "[1 for _ in [1] for y in y]",
        &["referenced before assignment", "`y`"],
    );
    assert::fails_skip_typecheck(
        "def f():\n [1 for _ in [1] for y in y]\nf()",
        &["referenced before assignment", "`y`"],
    );
}

#[test]
fn test_self_assign() {
    // Starlark spec is not clear whether it is allowed or not.
    assert::pass("x = [1,2]\na, x[0] = x");
    assert::pass("x = {0:0,1:1}\na, x[0] = x");
}

#[test]
fn test_nested_loops() {
    // Nested loops with returns used to cause problems in some cases, add a test
    assert::pass(
        r#"
def foo(y):
    for x in [1,2,3,4]:
        if x == 3:
            return y

def bar(xs):
    res = []
    for x in xs:
        if type(x) == type(1):
            fail("Type confusion")
        res.append(foo(x))
    assert_eq(xs, res)
bar(["a","b","c"])
"#,
    );
}

#[test]
fn test_label_assign() {
    // Test the a.b = c construct.
    // No builtin Starlark types support it, so we have to define a custom type (wapping a dictionary)

    #[derive(Debug, Trace, ProvidesStaticType, Display, NoSerialize, Allocative)]
    #[display("{:?}", self)]
    struct Wrapper<'v>(RefCell<SmallMap<String, Value<'v>>>);

    #[derive(Debug, Clone, Copy, StarlarkRepresentations)]
    enum WrapperView<'v> {
        Unfrozen(ValueTyped<'v, Wrapper<'v>>),
        Frozen(ValueTyped<'v, FrozenWrapper>),
    }

    #[starlark_value(type = "wrapper")]
    impl<'v> StarlarkType<'v> for WrapperView<'v> {
        fn get_attr(&self, attribute: &str, _heap: Heap<'v>) -> Option<Value<'v>> {
            match self {
                WrapperView::Unfrozen(this) => Some(*this.0.borrow().get(attribute).unwrap()),
                WrapperView::Frozen(_) => None,
            }
        }

        fn set_attr(&self, attribute: &str, new_value: Value<'v>) -> starlark::Result<()> {
            match self {
                WrapperView::Unfrozen(this) => {
                    this.0.borrow_mut().insert(attribute.to_owned(), new_value);
                    Ok(())
                }
                WrapperView::Frozen(_) => Err(starlark::Error::new_other(
                    ValueError::CannotMutateImmutableValue,
                )),
            }
        }
    }

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for Wrapper<'v> {
        type Type = WrapperView<'v>;
    }

    #[derive(
        Debug,
        ProvidesStaticType,
        Display,
        NoSerialize,
        Allocative,
        StarlarkPagable
    )]
    #[display("FrozenWrapper")]
    struct FrozenWrapper;

    #[starlark_value]
    impl<'v> StarlarkValue<'v> for FrozenWrapper {
        type Type = WrapperView<'v>;
    }

    impl<'v> Freeze<'v> for Wrapper<'v> {
        type Frozen<'fv> = FrozenWrapper;
        fn freeze<'fv>(self, _freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
            Ok(FrozenWrapper)
        }
    }

    #[starlark_module]
    fn module(builder: &mut GlobalsBuilder) {
        fn wrapper<'v>(heap: Heap<'v>) -> anyhow::Result<Value<'v>> {
            Ok(heap.alloc_complex(Wrapper(RefCell::new(SmallMap::new()))))
        }
    }

    let mut a = Assert::new();
    a.globals_add(module);
    a.pass(
        r#"
a = wrapper()
b = wrapper()
a.foo = 100
a.bar = 93
b.foo = 7
assert_eq(a.bar + b.foo, a.foo)

a.foo += 8
assert_eq(a.foo, 108)

count = []
def mk_wrapper():
    count.append(1)
    res = wrapper()
    res.x = 9
    return res

mk_wrapper().x += 5
assert_eq(len(count), 1)
"#,
    );
}

#[test]
fn test_self_mutate_list() {
    // Check functions that mutate and access self on lists
    let mut a = Assert::new();
    // TODO(nga): fix and enable.
    a.disable_static_typechecking();
    a.is_true(
        r#"
xs = [1, 2, 3]
xs.extend(xs)
xs == [1, 2, 3, 1, 2, 3]
"#,
    );
    a.is_true(
        r#"
xs = [1, 2, 3]
xs += xs
xs == [1, 2, 3, 1, 2, 3]
"#,
    );
    a.fail(
        r#"
xs = [1, 2, 3]
xs.pop(xs)
"#,
        "Type of parameter `index` doesn't match",
    );
    a.fail(
        r#"
xs = [1, 2, 3]
xs.remove(xs)
"#,
        "not found in list",
    );
    a.is_true(
        r#"
xs = [1, 2, 3]
xs.append(xs)
xs.remove(xs)
xs == [1, 2, 3]
"#,
    );
    a.is_true(
        r#"
xs = [1, 2, 3]
xs += xs
xs == [1, 2, 3, 1, 2, 3]
"#,
    );
    a.fail(
        r#"
xs = []
xs[xs]
"#,
        "Expected `int`, but got",
    );
    a.fail(
        r#"
xs = []
xs[xs] = xs
"#,
        "Expected `int`, but got",
    );
}

#[test]
fn test_list_slice_does_not_accept_bool() {
    // TODO(nga): this should fail.
    assert::fail("[1][False]", "Expected `int`, but got `bool");
}

#[test]
fn test_self_mutate_dict() {
    // Check functions that mutate and access self on dicts
    assert::is_true(
        r#"
xs = {1: 2}
xs |= xs
xs == {1: 2}
"#,
    );
    assert::fail(
        r#"
xs = {}
xs[xs]
"#,
        "not hashable",
    );
    assert::fail(
        r#"
xs = {}
xs[xs] = 1
"#,
        "not hashable",
    );
    assert::is_true(
        r#"
xs = {}
xs[1] = xs
len(xs[1]) == 1
"#,
    );
    assert::is_true(
        r#"
xs = {}
xs.update(xs)
len(xs) == 0
"#,
    );
}

#[test]
fn test_dict_union() {
    assert::is_true(
        r#"
xs = {1: 2, 3: 4}
xs |= {5: 6}
xs |= {1: 7}
xs.items() == [(1, 7), (3, 4), (5, 6)]
"#,
    );
}

#[test]
fn test_dict_with_frozen_list_key_inlined() {
    let mut a = Assert::new();
    a.module(
        "m.star",
        "\
li = []
def f():
    # This should fail at runtime.
    return {li: 1}
    ",
    );
    a.fail(
        "\
load('m.star', 'f')
f()
    ",
        "Value of type `list` is not hashable",
    );
}

#[test]
fn test_joe() {
    // Based on discussions at https://github.com/facebook/starlark-rust/issues/22
    let code = r#"
def animal(id):
    return {
        "kind": "giraffe",
        "name": "giraffe-%s" % id,
        "feeding": [
            {
                "name": "feeder",
                "image": "photos-%s" % id,
                "commands": [
                    "lift",
                    "roll-over",
                ],
            },
        ],
    }
animal("Joe")
"#;
    Module::with_temp_heap(|m| {
        let globals = Globals::standard();
        let mut eval = Evaluator::new(&m);
        let ast = AstModule::parse("code.bzl", code.to_owned(), &Dialect::Standard).unwrap();
        let res: Value = eval.eval_module(ast, &globals).unwrap();
        let animal: SmallMap<&str, Value> = ValueOf::<DictType<StarlarkStr, Value>>::new_err(res)
            .unwrap()
            .try_items::<&str, Value>()
            .collect::<crate::Result<_>>()
            .unwrap();
        println!("animal = {animal:?}");
        crate::Result::Ok(())
    })
    .unwrap();
}

#[test]
fn test_fuzzer_59102() {
    // From https://bugs.chromium.org/p/oss-fuzz/issues/detail?id=59102
    let src = "\"\u{e0070}";
    let res: Result<AstModule, crate::Error> =
        AstModule::parse("hello_world.star", src.to_owned(), &Dialect::Standard);
    // The panic actually only happens when we format the result
    let _unused = format!("{res:?}");
}

#[test]
fn test_fuzzer_59371() {
    // From https://bugs.chromium.org/p/oss-fuzz/issues/detail?id=59371
    let src = "\"\u{2009}\\x";
    let res: Result<AstModule, crate::Error> =
        AstModule::parse("hello_world.star", src.to_owned(), &Dialect::Standard);
    // The panic actually only happens when we format the result
    let _unused = format!("{res:?}");
}

#[test]
fn test_fuzzer_59839() {
    // From https://bugs.chromium.org/p/oss-fuzz/issues/detail?id=59839
    let src = "\"{20000000000000000396}\".format()";
    let ast = AstModule::parse("hello_world.star", src.to_owned(), &Dialect::Standard).unwrap();
    let globals: Globals = Globals::standard();
    Module::with_temp_heap(|module| {
        let mut eval: Evaluator = Evaluator::new(&module);
        assert!(eval.eval_module(ast, &globals).is_err());
        crate::Result::Ok(())
    })
    .unwrap();
}
