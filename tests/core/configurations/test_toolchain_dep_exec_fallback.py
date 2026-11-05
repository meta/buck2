# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class ToolchainDepExecFallbackTest(BuckTestCase):
    @buck_test()
    async def test_toolchain_dep_with_resolved_candidate(self, buck: Buck) -> None:
        # The toolchain dep finds the parent's exec platform among the candidates.
        result = await buck.cquery("deps(root//:resolves)")
        assert "root//:tc (root//:p#" in result.stdout

    @buck_test()
    async def test_toolchain_dep_with_fallback_platform(self, buck: Buck) -> None:
        # Every candidate is skipped, so resolution returns the fallback `ExecutionPlatformInfo`
        # (sunos); the toolchain dep inherits it.
        result = await buck.cquery(
            "deps(root//:falls_back)",
            "-c",
            "build.execution_platforms=root//execution_platforms:execution_platforms_platform_fallback",
        )
        assert "root//:tc (root//:p#" in result.stdout
        assert "sunos" in result.stdout

    @buck_test()
    async def test_toolchain_dep_with_unspecified_fallback(self, buck: Buck) -> None:
        # With `fallback` unset the parent proceeds with the unspecified exec platform, and so
        # does its toolchain dep.
        result = await buck.cquery("deps(root//:falls_back)")
        assert "root//:tc (root//:p#" in result.stdout
        assert "unspecified_exec" in result.stdout
