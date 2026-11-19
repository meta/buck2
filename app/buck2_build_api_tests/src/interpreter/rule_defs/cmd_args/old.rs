/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Tests for old/method-based cmd_args APIs.

use buck2_interpreter_for_build::interpreter::testing::expect_error;
use indoc::indoc;

use crate::interpreter::rule_defs::cmd_args::testing::tester;

#[test]
fn test_relative_absolute_old() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let contents = indoc!(
        r#"
        def test():
            args = cmd_args(absolute_prefix="$ABSOLUTE/", absolute_suffix="!")
            args.add(source_artifact("foo","bar/baz/qux.h"))
            args.relative_to(source_artifact("foo", "bar/foo"))

            assert_eq(get_args(args), ["$ABSOLUTE/../baz/qux.h!"])

            args = cmd_args()
            args.add(source_artifact("foo","bar/baz/qux.h"))
            args.relative_to(source_artifact("foo", "bar/baz"), parent=1)
            assert_eq(get_args(args), ["baz/qux.h"])
            "#
    );
    tester.run_starlark_bzl_test(contents)?;
    Ok(())
}

/// `relative_to` on the outer `cmd_args` does not apply to artifacts that were
/// added directly to the outer (only to nested `cmd_args` items).
#[test]
fn test_relative_to_does_not_affect_new_artifacts() -> buck2_error::Result<()> {
    let mut tester = tester().unwrap();
    let content = indoc!(
        r#"
        def test():
            args = cmd_args(
                source_artifact("foo", "bar.h"),
                relative_to=(source_artifact("foo", "baz.c"), 1),
            )
            # Self check
            assert_eq(get_args(args), ["./bar.h"])

            args2 = cmd_args(args, source_artifact("foo", "bar2.h"))
            assert_eq(get_args(args2), ["./bar.h", "foo/bar2.h"])
        "#
    );
    tester.run_starlark_bzl_test(content)?;
    Ok(())
}

#[test]
fn test_parent_error_negative() -> buck2_error::Result<()> {
    let mut tester = tester()?;
    let bad_count = indoc!(
        r#"
        def test():
            args = cmd_args(
                source_artifact("foo","qux.h"),
                parent=-12,
            )
            get_args(args)
        "#
    );
    expect_error(
        tester.run_starlark_bzl_test(bad_count),
        bad_count,
        "Integer value is too big to fit in u32",
    );
    Ok(())
}
