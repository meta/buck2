/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::provider::callable::register_provider;
use buck2_build_api::interpreter::rule_defs::register_rule_defs;
use buck2_core::bzl::ImportPath;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use indoc::indoc;

fn provider_tester() -> Tester {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_rule_defs);
    tester.additional_globals(register_provider);
    tester
}

#[test]
fn creates_providers() -> buck2_error::Result<()> {
    // TODO(nmj): Starlark doesn't let you call 'new_invoker()' on is_mutable types.
    //                 Once that's fixed, make sure we can call 'FooInfo' before the module is
    //                 frozen.
    let mut tester = provider_tester();

    tester.run_starlark_test(indoc!(
        r#"
    FooInfo = provider(fields=["bar", "baz"])
    FooInfo2 = FooInfo
    #frozen_foo_1 = FooInfo(bar="bar_f1", baz="baz_f1")
    #frozen_foo_2 = FooInfo(bar="bar_f2")

    assert_eq('provider(fields={"f1": provider_field(typing.Any, default=None)})', repr(provider(fields=["f1"])))
    assert_eq('provider[FooInfo](fields={"bar": provider_field(typing.Any, default=None), "baz": provider_field(typing.Any, default=None)})', repr(FooInfo))
    assert_eq('provider[FooInfo](fields={"bar": provider_field(typing.Any, default=None), "baz": provider_field(typing.Any, default=None)})', repr(FooInfo2))

    def test():
        assert_eq('provider[FooInfo](fields={"bar": provider_field(typing.Any, default=None), "baz": provider_field(typing.Any, default=None)})', repr(FooInfo))
        assert_eq('provider[FooInfo](fields={"bar": provider_field(typing.Any, default=None), "baz": provider_field(typing.Any, default=None)})', repr(FooInfo2))

        #assert_eq("FooInfo(bar=\"bar_f1\", baz=\"baz_f1\")", repr(frozen_foo1))
        #assert_eq("bar_f1", frozen_foo1.bar)
        #assert_eq("baz_f1", frozen_foo1.baz)
        #assert_eq("FooInfo(bar=\"bar_f2\", baz=None)", repr(frozen_foo2))
        #assert_eq("bar_f2", frozen_foo2.bar)
        #assert_eq(None, frozen_foo2.baz)

        foo_1 = FooInfo(bar="bar_1", baz="baz_1")
        foo_2 = FooInfo(bar="bar_2")

        assert_eq('provider[FooInfo](fields={"bar": provider_field(typing.Any, default=None), "baz": provider_field(typing.Any, default=None)})', repr(FooInfo))
        assert_eq("FooInfo(bar=\"bar_1\", baz=\"baz_1\")", repr(foo_1))
        assert_eq("bar_1", foo_1.bar)
        assert_eq("baz_1", foo_1.baz)
        assert_eq("FooInfo(bar=\"bar_2\", baz=None)", repr(foo_2))
        assert_eq("bar_2", foo_2.bar)
        assert_eq(None, foo_2.baz)

        assert_eq("{\"bar\":\"bar_1\",\"baz\":\"baz_1\"}", json.encode(foo_1))
    "#
    ))?;

    tester.run_starlark_test_expecting_error(
        indoc!(
            r#"
    FooInfo = provider(fields=["bar", "baz"])

    def test_compile_time():
        foo_1 = FooInfo(bar="bar1")
        foo_1.quz

    def test():
        pass
    "#
        ),
        "The attribute `quz` is not available on the type `FooInfo`",
    );

    tester.run_starlark_test_expecting_error(
        indoc!(
            r#"
    list = []
    list.append(provider(fields=["bar", "baz"]))
    "#
        ),
        "must be assigned to a variable",
    );

    // Make sure that frozen UserProvider instances work
    let mut tester = provider_tester();
    tester.add_import(
        &ImportPath::testing_new("root//provider:def1.bzl"),
        indoc!(
            r#"
            FooInfo = provider(fields=["foo"])
            "#
        ),
    )?;
    tester.add_import(
        &ImportPath::testing_new("root//provider:def2.bzl"),
        indoc!(
            r#"
            load("//provider:def1.bzl", "FooInfo")
            foo = FooInfo(foo="foo1")
            "#
        ),
    )?;
    tester.run_starlark_test(indoc!(
        r#"
        load("//provider:def2.bzl", "foo")
        def test():
            assert_eq('FooInfo(foo="foo1")', repr(foo))
        "#
    ))?;

    Ok(())
}

#[test]
fn test_builtin_provider_as_type() {
    let mut tester = provider_tester();
    tester
        .run_starlark_bzl_test(indoc!(
            r#"
        def test():
            configuration_info = ConfigurationInfo(constraints={}, values={})
            assert_true(isinstance(configuration_info, ConfigurationInfo))
            assert_true(isinstance(configuration_info, Provider))
            assert_true(not isinstance(configuration_info, DefaultInfo))
        "#
        ))
        .unwrap();
}

#[test]
fn test_user_defined_provider_as_type() {
    let mut tester = provider_tester();
    tester
        .run_starlark_bzl_test(indoc!(
            r#"
            FooInfo = provider(fields=["foo"])
            def test():
                foo = FooInfo(foo="foo1")
                assert_true(isinstance(foo, FooInfo))
                assert_true(isinstance(foo, Provider))
                assert_true(not isinstance(foo, DefaultInfo))
            "#
        ))
        .unwrap();
}

#[test]
fn test_user_defined_provider_creator_typecheck() {
    let mut tester = provider_tester();
    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
                Pkg = provider(fields = ["x"])

                def create_pkg():
                    return Pkg(y = 1)

                def test():
                    pass
    "#
        ),
        "Unexpected parameter named `y`",
    );
}

#[test]
fn test_provider_non_unique_fields() {
    let mut tester = provider_tester();
    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
                Pkg = provider(fields = ["x", "x"])

                def test():
                    pass
            "#
        ),
        "non-unique field names",
    )
}

/// `providers` of a dependency attribute lists provider types.
#[test]
fn test_dep_attr_providers_are_provider_types() {
    let mut tester = provider_tester();
    tester
        .run_starlark_bzl_test(indoc!(
            r#"
            FooInfo = provider(fields = ["x"])
            attrs.dep(providers = [DefaultInfo, FooInfo])
            def test():
                pass
            "#
        ))
        .unwrap();
    tester.run_starlark_bzl_test_expecting_error(
        "attrs.dep(providers = [DefaultInfo, 1])\n",
        "Type of parameter `providers` doesn't match, expected `list[ProviderCallable] | tuple[ProviderCallable, ...]`, actual `list (repr: [DefaultInfo, 1])`",
    );
}
