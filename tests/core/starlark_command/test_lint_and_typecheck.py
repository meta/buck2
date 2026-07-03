# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import os
import shutil

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class LintAndTypecheckTest(BuckTestCase):
    @buck_test()
    async def test_lint_fails(self, buck: Buck) -> None:
        await expect_failure(
            buck.starlark("lint", "bad_warning.bzl"),
            stderr_regex="Found 3 lints",
        )

    @buck_test()
    async def test_exclude(self, buck: Buck) -> None:
        for d in ["dir/keep", "dir/skip"]:
            (buck.cwd / d).mkdir(parents=True)
            shutil.copy(buck.cwd / "bad_warning.bzl", buck.cwd / d)
        await expect_failure(buck.starlark("lint", "dir"), stderr_regex="Found 6 lints")
        await expect_failure(
            buck.starlark("lint", "dir", "-X", "dir/skip"), stderr_regex="Found 3 lints"
        )
        await buck.starlark(
            "lint", "dir", "-X", "dir/skip", "-X", "dir/keep/bad_warning.bzl"
        )

    @buck_test()
    async def test_typecheck_fails(self, buck: Buck) -> None:
        await buck.starlark("typecheck", "good.bzl")
        await expect_failure(
            buck.starlark("typecheck", "bad.bzl"),
            stderr_regex="Detected 2 errors",
        )

    @buck_test()
    async def test_explicit_path_goes_through_the_path_check(self, buck: Buck) -> None:
        await expect_failure(
            buck.starlark("lint", "nope.bzl"),
            stderr_regex="File `nope.bzl` does not exist",
        )
        os.symlink(".", buck.cwd / "linkdir")
        await expect_failure(
            buck.starlark("lint", "linkdir/good.bzl"),
            stderr_regex="is behind the symlink `linkdir`, which buck2 does not follow",
        )

    @buck_test()
    async def test_typecheck_reports_unresolved_append(self, buck: Buck) -> None:
        # A typo'd `y.append(1)` inside a def is a scope error, not a daemon panic.
        failure = await expect_failure(
            buck.starlark("typecheck", "unresolved_append.bzl"),
            stderr_regex="Detected 1 errors",
        )
        assert "Variable `y` not found" in failure.stdout, failure.stdout

    @buck_test()
    async def test_typecheck_reports_duplicate_argument(self, buck: Buck) -> None:
        # A parameter bound both positionally and by name is a typing error naming it, as at
        # runtime, not an internal error; a union of callables where another alternative accepts
        # the call is accepted.
        failure = await expect_failure(
            buck.starlark("typecheck", "duplicate_arg.bzl"),
            stderr_regex="Detected 1 errors",
        )
        assert "Argument `a` occurs more than once" in failure.stdout, failure.stdout
        assert "Internal error" not in failure.stdout, failure.stdout
        await buck.starlark("typecheck", "duplicate_arg_union.bzl")

    @buck_test()
    async def test_typecheck_accepts_int_float_membership(self, buck: Buck) -> None:
        # `in` and dict indexing widen int and float like `==` does.
        await buck.starlark("typecheck", "numeric_membership.bzl")

    @buck_test()
    async def test_typecheck_bundled_cell(self, buck: Buck) -> None:
        # Files in bundled external cells do not exist on disk; check that they
        # are read through the file ops that know how to materialize them.
        await buck.starlark("typecheck", "uses_prelude.bzl")
        await buck.starlark("typecheck", "nano_prelude/prelude.bzl")

    @buck_test()
    async def test_lint_bundled_cell(self, buck: Buck) -> None:
        await buck.starlark("lint", "nano_prelude")
