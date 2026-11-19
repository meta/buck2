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
use buck2_build_api::interpreter::rule_defs::provider::collection::tester::collection_creator;
use buck2_build_api::interpreter::rule_defs::register_rule_defs;
use buck2_core::bzl::ImportPath;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::interpreter::testing::expect_error;
use indoc::indoc;

use crate::interpreter::rule_defs::artifact::testing::artifactory;

fn provider_collection_tester() -> buck2_error::Result<Tester> {
    let mut tester = Tester::new()?;
    tester.additional_globals(collection_creator);
    tester.additional_globals(artifactory);
    tester.additional_globals(register_rule_defs);
    tester.additional_globals(register_provider);
    tester.add_import(
        &ImportPath::testing_new("root//provider:defs1.bzl"),
        indoc!(
            r#"
                    FooInfo = provider(fields=["foo"])
                    BarInfo = provider(fields=["bar"])
                    BazInfo = provider(fields=["baz"])
                    "#
        ),
    )?;
    tester.add_import(
        &ImportPath::testing_new("root//provider:defs2.bzl"),
        indoc!(
            r#"
                    load("//provider:defs1.bzl", "FooInfo", "BarInfo", "BazInfo")
                    foo1 = FooInfo(foo="foo1")
                    foo2 = FooInfo(foo="foo2")
                    bar1 = BarInfo(bar="bar1")
                    baz1 = BazInfo(baz="baz1")
                    "#
        ),
    )?;

    Ok(tester)
}

#[test]
fn provider_collection_constructs_properly() -> buck2_error::Result<()> {
    let mut tester = provider_collection_tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
            load("//provider:defs1.bzl", "FooInfo", "BarInfo", "BazInfo")
            load("//provider:defs2.bzl", "foo1", "bar1")
            def test():
                col = create_collection([foo1, bar1, DefaultInfo()])
                assert_eq(None, col.get(BazInfo))
                assert_eq("foo1", col[FooInfo].foo)
                assert_eq("bar1", col[BarInfo].bar)
                assert_eq([], col[DefaultInfo].default_outputs)
            "#
    ))?;
    Ok(())
}

#[test]
fn provider_collection_fails_to_construct_on_bad_data() -> buck2_error::Result<()> {
    let mut tester = provider_collection_tester()?;
    let not_a_list = indoc!(
        r#"
            load("//provider:defs2.bzl", "foo1")
            def test():
                create_collection(foo1)
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(not_a_list),
        not_a_list,
        "expected a list of Provider objects",
    );

    let mut tester = provider_collection_tester()?;
    let not_a_provider = indoc!(
        r#"
            load("//provider:defs2.bzl", "foo1", "bar1")
            def test():
                create_collection([foo1, bar1, "not a provider", DefaultInfo()])
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(not_a_provider),
        not_a_provider,
        "expected a Provider object",
    );

    let mut tester = provider_collection_tester()?;
    let duplicate_provider_types = indoc!(
        r#"
            load("//provider:defs2.bzl", "foo1", "foo2", "bar1")
            def test():
                create_collection([foo1, bar1, foo2, DefaultInfo()])
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(duplicate_provider_types),
        duplicate_provider_types,
        "specified twice",
    );

    let mut tester = provider_collection_tester()?;
    let missing_default_info = indoc!(
        r#"
            load("//provider:defs2.bzl", "foo1", "bar1")
            def test():
                create_collection([foo1, bar1])
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(missing_default_info),
        missing_default_info,
        "did not receive a `DefaultInfo`",
    );
    Ok(())
}

#[test]
fn provider_collection_at_reports_the_missing_provider() -> buck2_error::Result<()> {
    let mut tester = provider_collection_tester()?;
    let missing_provider = indoc!(
        r#"
            load("//provider:defs1.bzl", "BazInfo")
            load("//provider:defs2.bzl", "foo1", "bar1")
            def test():
                col = create_collection([foo1, bar1, DefaultInfo()])
                col[BazInfo]
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(missing_provider),
        missing_provider,
        "does not have a key `BazInfo`",
    );
    Ok(())
}

#[test]
fn returns_default_info() -> buck2_error::Result<()> {
    let mut tester = provider_collection_tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
            artifact = source_artifact("foo", "bar.cpp")
            frozen_collection = create_collection([
                DefaultInfo(
                    sub_targets={"foo": []},
                    default_outputs=[artifact]
                )
            ])
            def test():
                di_sub_targets = get_default_info_sub_targets(frozen_collection)
                di_default_outputs = get_default_info_default_outputs(frozen_collection)
                assert_eq([], di_sub_targets["foo"][DefaultInfo].default_outputs)
                assert_eq([artifact], di_default_outputs)
            "#
    ))
}

#[test]
fn provider_collection_contains_methods_and_in_operator() -> buck2_error::Result<()> {
    let mut tester = provider_collection_tester()?;
    tester.add_import(
        &ImportPath::testing_new("root//providers:defs.bzl"),
        indoc!(
            r#"
                FooInfo = provider(fields=["foo"])
                BarInfo = provider(fields=["bar"])
                "#
        ),
    )?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
            load("//providers:defs.bzl", "FooInfo", "BarInfo")
            c1 = create_collection([DefaultInfo(), FooInfo(foo="f1")])
            c2 = create_collection([DefaultInfo(), FooInfo(foo="f2"), BarInfo(bar="b2")])
            def test():
                assert_eq(True, contains_provider(c1, FooInfo))
                assert_eq(True, FooInfo in c1)
                assert_eq(False, contains_provider(c1, BarInfo))
                assert_eq(False, BarInfo in c1)
                assert_eq(["DefaultInfo", "FooInfo", "BarInfo"], providers_list(c2))
            "#
    ))
}

#[test]
fn provider_collection_get() -> buck2_error::Result<()> {
    let mut tester = provider_collection_tester()?;
    tester.add_import(
        &ImportPath::testing_new("root//providers:defs.bzl"),
        indoc!(
            r#"
                FooInfo = provider(fields=["foo"])
                BarInfo = provider(fields=["bar"])

                # Frozen collection
                cf = create_collection([DefaultInfo(), FooInfo(foo="f1")])
                "#
        ),
    )?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
            load("//providers:defs.bzl", "FooInfo", "BarInfo", "cf")

            # Unfrozen collection
            cu = create_collection([DefaultInfo(), FooInfo(foo="f1")])

            def test():
                assert_eq("f1", cu.get(FooInfo).foo)
                assert_eq(None, cu.get(BarInfo))
                assert_eq("f1", cf.get(FooInfo).foo)
                assert_eq(None, cf.get(BarInfo))
            "#
    ))
}
