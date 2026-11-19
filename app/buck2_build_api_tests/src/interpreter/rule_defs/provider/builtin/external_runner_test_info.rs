/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::register_rule_defs;
use buck2_build_api::interpreter::rule_defs::required_test_local_resource::register_required_test_local_resource;
use buck2_core::bzl::ImportPath;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use indoc::indoc;

fn tester() -> Tester {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_rule_defs);
    tester.additional_globals(register_required_test_local_resource);
    tester
}

#[test]
fn test_construction() -> buck2_error::Result<()> {
    let test = indoc!(
        r#"
        def test():
            ExternalRunnerTestInfo(type = "foo")
            ExternalRunnerTestInfo(type = "foo", command = ["cmd"])
            ExternalRunnerTestInfo(type = "foo", command = ["cmd", cmd_args()])
            ExternalRunnerTestInfo(type = "foo", command = ("cmd",))
            ExternalRunnerTestInfo(type = "foo", env = {"foo": "bar" })
            ExternalRunnerTestInfo(type = "foo", env = {"foo": cmd_args() })
            ExternalRunnerTestInfo(type = "foo", labels = ["foo"])
            ExternalRunnerTestInfo(type = "foo", contacts = ["foo"])
            ExternalRunnerTestInfo(type = "foo", labels = ("foo",))
            ExternalRunnerTestInfo(type = "foo", use_project_relative_paths = True)
            ExternalRunnerTestInfo(type = "foo", run_from_project_root = True)
            ExternalRunnerTestInfo(type = "foo", local_resources = {"bar": None}, required_local_resources = [RequiredTestLocalResource("bar", listing=False)])
        "#
    );
    let mut tester = tester();
    tester.run_starlark_bzl_test(test)?;
    Ok(())
}

#[test]
fn test_validation() -> buck2_error::Result<()> {
    let mut tester = tester();
    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo()
        "#
        ),
        "`type`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = 123)
        "#
        ),
        "Expected type `str` but got `int`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", command = "foo")
        "#
        ),
        "Expected type `None | list[Artifact",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", command = [123])
        "#
        ),
        "but got `list[int]`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", env = "foo")
        "#
        ),
        "Expected type `None | dict[str, Artifact",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", env = {"foo": 123})
        "#
        ),
        "but got `dict[str, int]`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", labels = "foo")
        "#
        ),
        "Expected type `None | list[str] | tuple[str, ...]` but got `str`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", labels = [123])
        "#
        ),
        "but got `list[int]`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", contacts = "foo")
        "#
        ),
        "Expected type `None | list[str] | tuple[str, ...]` but got `str`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", contacts = [123])
        "#
        ),
        "but got `list[int]`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", use_project_relative_paths = "foo")
        "#
        ),
        "Expected type `None | bool` but got `str`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", run_from_project_root = "foo")
        "#
        ),
        "Expected type `None | bool` but got `str`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", default_executor = "foo")
        "#
        ),
        "Expected type `CommandExecutorConfig | None` but got `str`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", executor_overrides = {"foo": "bar" })
        "#
        ),
        "Expected type `None | dict[str, CommandExecutorConfig]` but got `dict[str, str]`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", required_local_resources = ["bar"])
        "#
        ),
        "Expected type `None | list[RequiredTestLocalResource] | tuple[RequiredTestLocalResource, ...]` but got `list[str]`",
    );

    tester.run_starlark_bzl_test_expecting_error(
        indoc!(
            r#"
        def test():
            ExternalRunnerTestInfo(type = "foo", required_local_resources = [RequiredTestLocalResource("bar")])
        "#
        ),
        "`required_local_resources` contains `bar` which is not present in `local_resources`",
    );

    Ok(())
}

#[test]
fn test_validation_at_freeze() -> buck2_error::Result<()> {
    let mut tester = tester();

    let res = tester.add_import(
        &ImportPath::testing_new("root//test:def1.bzl"),
        indoc!(
            r#"
        def make_info():
            contacts = []
            info = ExternalRunnerTestInfo(type = "foo", contacts = contacts)
            contacts.append(123)
            return info

        exported_info = make_info()
        "#
        ),
    );

    assert!(res.is_err());

    Ok(())
}
