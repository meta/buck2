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
        # Rule names in build files come from the implicit prelude import; rule
        # calls are typechecked against their attribute-derived signatures.
        await buck.starlark("typecheck", "TARGETS.fixture")
        await expect_failure(
            buck.starlark("typecheck", "bad/TARGETS.fixture"),
            stderr_regex="Detected 1 errors",
        )

    @buck_test()
    async def test_typecheck_implicit_prelude_symbols_in_bzl(self, buck: Buck) -> None:
        await buck.starlark("typecheck", "implicit.bzl")
