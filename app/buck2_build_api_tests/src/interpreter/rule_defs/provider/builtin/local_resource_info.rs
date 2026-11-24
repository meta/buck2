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
use buck2_core::bzl::ImportPath;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::interpreter::testing::expect_error;
use buck2_interpreter_for_build::label::testing::label_creator;
use indoc::indoc;

fn new_tester() -> Tester {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(label_creator);
    tester.additional_globals(register_rule_defs);
    tester
}

#[test]
fn test_construction() -> buck2_error::Result<()> {
    let mut tester = new_tester();
    let test = indoc!(
        r#"
        def test():
            LocalResourceInfo(setup=["/foo", "--resource"], resource_env_vars={"RESOURCE_ENV_VAR": "json_key"})
            LocalResourceInfo(setup=cmd_args(["/foo", "--resource"]), resource_env_vars={"RESOURCE_ENV_VAR": "json_key"})
            LocalResourceInfo(setup=cmd_args(["/foo", "--resource"]), resource_env_vars={"RESOURCE_ENV_VAR": "json_key"}, setup_timeout_seconds=10)
            LocalResourceInfo(setup=cmd_args(["/foo", "--resource"]), resource_env_vars={"RESOURCE_ENV_VAR": "json_key"}, setup_timeout_seconds=10.5)
        "#
    );
    tester.run_starlark_bzl_test(test)?;
    Ok(())
}

#[test]
fn test_missing_fields_validation() -> buck2_error::Result<()> {
    let mut tester = new_tester();
    {
        let test = indoc!(
            r#"
            def test():
                LocalResourceInfo(resource_env_vars={})
            "#
        );
        expect_error(tester.run_starlark_bzl_test(test), test, "`setup`");
    }
    {
        let test = indoc!(
            r#"
            def test():
                LocalResourceInfo(setup=cmd_args())
            "#
        );
        expect_error(
            tester.run_starlark_bzl_test(test),
            test,
            "`resource_env_vars`",
        );
    }
    Ok(())
}

#[test]
fn test_validation_1() {
    let mut tester = new_tester();

    let test = indoc!(
        r#"
        def test():
            wrong_setup = {5:6}
            LocalResourceInfo(setup=wrong_setup, resource_env_vars={"RESOURCE_ENV_VAR": "json_key"})
        "#
    );
    expect_error(
        tester.run_starlark_bzl_test(test),
        test,
        "Expected type `CmdArgsInput` but got",
    );
}

#[test]
fn test_validation_2() {
    let mut tester = new_tester();
    let test = indoc!(
        r#"
            def test():
                wrong_setup = []
                LocalResourceInfo(setup=wrong_setup, resource_env_vars={"RESOURCE_ENV_VAR": "json_key"})
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(test),
        test,
        "Value for `setup` field is an empty command line",
    );
}

#[test]
fn test_validation_3() {
    let mut tester = new_tester();
    let test = indoc!(
        r#"
            def test():
                wrong_env_vars = "baz"
                LocalResourceInfo(setup=["/foo", "--resource"], resource_env_vars=wrong_env_vars)
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(test),
        test,
        "Expected type `dict[str, str]`",
    );
}

#[test]
fn test_validation_4() {
    let mut tester = new_tester();
    let test = indoc!(
        r#"
            def test():
                wrong_env_vars = {}
                LocalResourceInfo(setup=["/foo", "--resource"], resource_env_vars=wrong_env_vars)
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(test),
        test,
        "Value for `resource_env_vars` field is an empty dictionary",
    );
}

#[test]
fn test_validation_5() {
    let mut tester = new_tester();
    let test = indoc!(
        r#"
            def test():
                wrong_env_vars = {1:"one"}
                LocalResourceInfo(setup=["/foo", "--resource"], resource_env_vars=wrong_env_vars)
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(test),
        test,
        "Expected type `dict[str, str]`",
    );
}

#[test]
fn test_validation_6() {
    let mut tester = new_tester();
    let test = indoc!(
        r#"
            def test():
                wrong_env_vars = {"one":1}
                LocalResourceInfo(setup=["/foo", "--resource"], resource_env_vars=wrong_env_vars)
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(test),
        test,
        "Expected type `dict[str, str]`",
    );
}

#[test]
fn test_validation_7() {
    let mut tester = new_tester();
    let test = indoc!(
        r#"
            def test():
                wrong_env_vars = {"one":"1"}
                LocalResourceInfo(setup=["/foo", "--resource"], resource_env_vars=wrong_env_vars, setup_timeout_seconds="42")
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(test),
        test,
        "Expected type `None | float | int`",
    );
}

#[test]
fn test_validation_at_freeze() -> buck2_error::Result<()> {
    let mut tester = new_tester();
    let test = indoc!(
        r#"
        def make_info():
            resource_env_vars = {"RESOURCE_ENV_VAR": "json_key"}
            info = LocalResourceInfo(setup=["/foo", "--resource"], resource_env_vars=resource_env_vars)
            resource_env_vars["ONE"] = 1
            return info

        exported_info = make_info()
        "#
    );
    let res = tester.add_import(&ImportPath::testing_new("root//:defs2.bzl"), test);
    assert!(res.is_err());
    Ok(())
}
