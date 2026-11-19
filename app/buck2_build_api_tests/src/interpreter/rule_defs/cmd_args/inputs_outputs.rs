/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::bzl::ImportPath;
use indoc::indoc;

use crate::interpreter::rule_defs::cmd_args::testing::tester;

#[test]
fn test_inputs_outputs() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        artifact1 = source_artifact("foo", "bar/baz.h")
        artifact2 = bound_artifact("//:dep1", "dir/baz.h")

        def norm(xs):
            # Try and make everything consistent for the equality test
            return sorted([repr(x) for x in xs])

        def test():
            artifact3 = source_artifact("foo", "bar/quz.h")
            artifact4 = bound_artifact("//:dep2", "dir/quz.h")
            artifact5 = declared_artifact("declared")

            cli = cmd_args(hidden=artifact1)
            cli.add(artifact3)
            cli.add("just a string")
            cli.add(artifact4)
            cli.add(artifact5.as_output())

            assert_eq(make_inputs([artifact3, artifact4, artifact1]), cli.inputs)
            assert_eq(3, len(cli.inputs))
            assert_eq(norm([artifact5.as_output()]), norm(cli.outputs))
        "#
    ))?;
    Ok(())
}

#[test]
fn test_inputs_equality_ignores_order() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            a = source_artifact("foo", "a.h")
            b = source_artifact("foo", "b.h")
            c = source_artifact("foo", "c.h")

            inputs = cmd_args(a, b).inputs
            assert_eq(inputs, cmd_args(b, a).inputs)
            assert_eq(inputs, cmd_args(b, hidden = [a, b]).inputs)
            assert_eq(False, inputs == cmd_args(a).inputs)
            assert_eq(False, inputs == cmd_args(a, c).inputs)
        "#
    ))?;
    Ok(())
}

#[test]
fn test_ignore_artifacts() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def norm(xs):
            # Try and make everything consistent for the equality test
            return sorted([repr(x) for x in xs])

        def test():
            artifact = bound_artifact("//:dep2", "dir/quz.h")

            cli = cmd_args(ignore_artifacts=True)
            cli.add(artifact)

            assert_eq(make_inputs([]), cli.inputs)
            assert_eq([], cli.outputs)

            assert_eq_ignore_hash(["buck-out/v2/art/root/<HASH>/__dep2__/dir/quz.h"], get_args(cli))
        "#
    ))?;
    Ok(())
}

#[test]
fn test_salt_hidden_is_a_dependency() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            artifact = bound_artifact("//:dep2", "dir/quz.h")

            cli = cmd_args(hidden = artifact, experimental_salt_hidden = True)
            assert_eq(make_inputs([artifact]), cli.inputs)
            assert_eq([], get_args(cli))

            # Everything reached through the salted hidden list is covered,
            # including nested hidden lists and rendered items below it.
            nested = cmd_args(hidden = cmd_args(hidden = artifact), experimental_salt_hidden = True)
            assert_eq(make_inputs([artifact]), nested.inputs)
            with_ignore = cmd_args(hidden = artifact, ignore_artifacts = True, experimental_salt_hidden = True)
            assert_eq(make_inputs([artifact]), with_ignore.inputs)
        "#
    ))?;
    Ok(())
}

#[test]
fn test_ignore_artifacts_hidden_static_path_is_not_a_dependency() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            artifact = bound_artifact("//:dep2", "dir/quz.h")
            assert_eq(make_inputs([]), cmd_args(hidden = artifact, ignore_artifacts = True).inputs)
        "#
    ))?;
    Ok(())
}

#[test]
fn test_ignore_artifacts_under_enclosing_hidden_is_not_a_dependency() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            artifact = bound_artifact("//:dep2", "dir/quz.h")

            # A hidden argument list may exempt one path from being a dependency.
            assert_eq(make_inputs([]), cmd_args(hidden = cmd_args(artifact, ignore_artifacts = True)).inputs)
            assert_eq(make_inputs([]), cmd_args(hidden = [cmd_args(artifact, ignore_artifacts = True), "x"]).inputs)
        "#
    ))?;
    Ok(())
}

#[test]
fn test_frozen_inputs_outputs() -> buck2_error::Result<()> {
    let mut tester = tester()?;

    tester.add_import(
        &ImportPath::testing_new("root//test:def1.bzl"),
        indoc!(
            r#"
            def norm(xs):
                # Try and make everything consistent for the equality test
                return sorted([repr(x) for x in xs])

            input = source_artifact("foo", "bar/quz.h")
            output = declared_bound_artifact("//foo:bar", "declared")

            cli = cmd_args()
            cli.add("string")
            cli.add(input)
            cli.add(output.as_output())

            expected_outputs = norm([output.as_output()])
            expected_inputs = make_inputs([input])
            "#
        ),
    )?;

    tester.run_starlark_bzl_test(indoc!(
        r#"
        load("//test:def1.bzl", "cli", "expected_inputs", "expected_outputs", "norm")

        def test():
            assert_eq(expected_inputs, cli.inputs)
            assert_eq(expected_outputs, norm(cli.outputs))
        "#
    ))?;
    Ok(())
}
