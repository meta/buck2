/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_interpreter_for_build::interpreter::testing::expect_error;
use indoc::indoc;

use crate::interpreter::rule_defs::cmd_args::testing::tester;

#[test]
fn stringifies_correctly() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        artifact1 = source_artifact("foo", "bar/baz.h")
        artifact2 = bound_artifact("//:dep1", "dir/baz.h")
        arg2 = "string1"
        arg3 = artifact1
        arg4 = artifact2
        arg5 = label("//foo:bar[baz]")

        def test():
            artifact3 = source_artifact("foo", "bar/quz.h")
            artifact4 = bound_artifact("//:dep2", "dir/quz.h")
            arg7 = "string2"
            arg8 = artifact3
            arg9 = artifact4
            arg10 = label("//foo:bar[quz]")

            assert_eq("string1", stringify_cli_arg(arg2))
            assert_eq("foo/bar/baz.h", stringify_cli_arg(arg3))
            assert_eq_ignore_hash("buck-out/v2/art/root/<HASH>/__dep1__/dir/baz.h", stringify_cli_arg(arg4))
            assert_eq_ignore_hash("root//foo:bar[baz] (<testing>#<HASH>)", stringify_cli_arg(arg5))
            assert_eq("string2", stringify_cli_arg(arg7))
            assert_eq("foo/bar/quz.h", stringify_cli_arg(arg8))
            assert_eq_ignore_hash("buck-out/v2/art/root/<HASH>/__dep2__/dir/quz.h", stringify_cli_arg(arg9))
            assert_eq_ignore_hash("root//foo:bar[quz] (<testing>#<HASH>)", stringify_cli_arg(arg10))
        "#
    ))?;

    let contents = indoc!(
        r#"
        def test():
            arg = stringify_cli_arg(["list of strings aren't valid"])
        "#
    );

    expect_error(
        tester.run_starlark_bzl_test(contents),
        contents,
        "Expected `Artifact | CellPath | CellRoot",
    );

    Ok(())
}

#[test]
fn displays_correctly() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            cli = cmd_args(format="x{}y", quote="shell", hidden="bar")
            cli.add("foo")
            # TODO(nga): fix options formatting.
            assert_eq('cmd_args("foo", hidden=["bar"], format="x{}y", quote="shell")', str(cli))
            assert_eq('cmd_args(\n  "foo",\n  hidden=[ "bar" ],\n  format="x{}y",\n  quote="shell"\n)', prepr(cli))
        "#
    ))?;

    Ok(())
}

#[test]
fn identity_format_is_dropped() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            assert_eq('cmd_args()', str(cmd_args(format = "{}")))
            assert_eq('cmd_args("foo")', str(cmd_args("foo", format = "{}")))
            assert_eq('cmd_args(format="{}x")', str(cmd_args(format = "{}x")))
        "#
    ))?;

    Ok(())
}

#[test]
fn displays_correctly_replace_regex() {
    let mut tester = tester().unwrap();
    tester
        .run_starlark_bzl_test(indoc!(
            r#"
        def test():
            cli = cmd_args(replace_regex=(regex("foo"), "bar"))
            assert_eq('cmd_args(replacements=[("foo", "bar")])', str(cli))
        "#
        ))
        .unwrap();
}

#[test]
fn command_line_builder() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let content = indoc!(
        r#"
        b1 = cmd_args()
        b2 = cmd_args()
        b3 = cmd_args()
        b4 = cmd_args()
        b5 = cmd_args()

        b2.add("b2 s1").add(["b2 s2"])
        b3.add("b3 s1").add(["b3 s2"])

        a1 = source_artifact("foo", "bar1/baz.h")
        a2 = bound_artifact("//:dep1", "dir/baz.h")
        a3 = source_artifact("foo", "bar2/baz.h")
        a4 = bound_artifact("//:dep2", "dir/baz.h")
        l1 = str(label("//foo:bar[baz]"))

        b1.add("b1 s1", "b1 s2").add(b2).add(a1).add(a2)
        b1.add([b3, l1]).add([a3, a4])

        # Add something so we can verify it doesn't modify b1
        b2 = b2.copy()
        b2.add("not in b1")

        # Ensure that both lists, and *args formats work
        b4.add(cmd_args(["--foo=", a1, ",bar=", b3, ",baz"], delimiter = ""))
        b4.add(cmd_args("--foo=", a1, ",bar=", b3, ",baz", delimiter = ""))

        # Make sure delimiters work properly
        b5.add(cmd_args("foo", delimiter = ""))
        b5.add(cmd_args("foo", "bar", delimiter=","))
        b5.add(cmd_args("foo", "bar", "baz", delimiter=","))

        mutable_b1_args = get_args(b1)
        mutable_b2_args = get_args(b2)
        mutable_b3_args = get_args(b3)
        mutable_b4_args = get_args(b4)
        mutable_b5_args = get_args(b5)

        def test():
            b1_args_expected = [
                "b1 s1",
                "b1 s2",
                "b2 s1",
                "b2 s2",
                "foo/bar1/baz.h",
                "buck-out/v2/art/root/<HASH>/__dep1__/dir/baz.h",
                "b3 s1",
                "b3 s2",
                "root//foo:bar[baz] (<testing>#<HASH>)",
                "foo/bar2/baz.h",
                "buck-out/v2/art/root/<HASH>/__dep2__/dir/baz.h",
            ]
            b2_args_expected = [
                "b2 s1",
                "b2 s2",
                "not in b1",
            ]
            b3_args_expected = [
                "b3 s1",
                "b3 s2",
            ]
            b4_args_expected = [
                "--foo=foo/bar1/baz.h,bar=b3 s1b3 s2,baz",
                "--foo=foo/bar1/baz.h,bar=b3 s1b3 s2,baz",
            ]
            b5_args_expected = [
                "foo",
                "foo,bar",
                "foo,bar,baz",
            ]

            # Check values while builders were mutable
            assert_eq_ignore_hash(b1_args_expected, mutable_b1_args)

            assert_eq_ignore_hash(b2_args_expected, mutable_b2_args)

            assert_eq_ignore_hash(b3_args_expected, mutable_b3_args)

            assert_eq_ignore_hash(b4_args_expected, mutable_b4_args)

            assert_eq_ignore_hash(b5_args_expected, mutable_b5_args)

            # Check that frozen values still work
            frozen_b1_args = get_args(b1)
            frozen_b2_args = get_args(b2)
            frozen_b3_args = get_args(b3)
            frozen_b4_args = get_args(b4)
            frozen_b5_args = get_args(b5)

            assert_eq_ignore_hash(b1_args_expected, frozen_b1_args)

            assert_eq_ignore_hash(b2_args_expected, frozen_b2_args)

            assert_eq_ignore_hash(b3_args_expected, frozen_b3_args)

            assert_eq_ignore_hash(b4_args_expected, frozen_b4_args)

            assert_eq_ignore_hash(b5_args_expected, frozen_b5_args)

            # Make sure we can add frozen CLIs to unfrozen ones
            b6 = cmd_args()
            b6.add("b6").add(b2).add([b3])

            b6_args_expected = ["b6"] + b2_args_expected + b3_args_expected
            assert_eq(b6_args_expected, get_args(b6))
        "#
    );

    tester.run_starlark_bzl_test(content)?;

    let content_invalid_type_1 = r#"cmd_args().add({"not": "an arg"})"#;
    let content_invalid_type_3 = r#"cmd_args().add([{"not": "an arg"}])"#;

    expect_error(
        tester.run_starlark_bzl_test(content_invalid_type_1),
        content_invalid_type_1,
        "Expected `Artifact | CellPath | CellRoot",
    );
    expect_error(
        tester.run_starlark_bzl_test(content_invalid_type_3),
        content_invalid_type_3,
        "Expected `Artifact | CellPath | CellRoot",
    );

    Ok(())
}
