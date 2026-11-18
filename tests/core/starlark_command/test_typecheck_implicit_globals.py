# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class TypecheckImplicitGlobalsTest(BuckTestCase):
    @buck_test()
    async def test_typecheck_build_file(self, buck: Buck) -> None:
        # Currently the oracle lacks the implicit prelude symbols, so even the
        # good build file fails with "not found" errors for the rule names.
        await expect_failure(
            buck.starlark("typecheck", "TARGETS.fixture"),
            stdout_regex="not found",
        )
        # The bad build file also fails, but with "not found" for the rule
        # name itself instead of the bad attribute.
        await expect_failure(
            buck.starlark("typecheck", "bad/TARGETS.fixture"),
            stdout_regex="not found",
        )

    @buck_test()
    async def test_typecheck_implicit_prelude_symbols_in_bzl(self, buck: Buck) -> None:
        # Currently `asserts` from the implicit prelude import is reported as not found.
        await expect_failure(
            buck.starlark("typecheck", "implicit.bzl"),
            stdout_regex="not found",
        )
