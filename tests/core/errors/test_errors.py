# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict
import re

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env
from buck2.tests.e2e_util.helper.golden import golden


class ErrorsTest(BuckTestCase):
    @buck_test()
    async def test_soft_error(self, buck: Buck) -> None:
        await expect_failure(
            buck.targets(":"),
            stderr_regex="starlark_raised_soft_error.*Will be reported",
        )

    @buck_test()
    @env("BUCK2_HARD_ERROR", "false")
    async def test_soft_error_is_silent(self, buck: Buck) -> None:
        res = await buck.targets(":", "no_stack:")
        assert "starlark_raised_soft_error" not in res.stderr
        assert "starlark_no_stack_soft_error" not in res.stderr

    @buck_test()
    @env("BUCK2_HARD_ERROR", "false")
    async def test_soft_error_no_stack(self, buck: Buck) -> None:
        await buck.targets(":")
        log = await buck.log("show")
        assert "Traceback" in log.stdout

        await buck.targets("no_stack:")
        log = await buck.log("show")
        assert "Traceback" not in log.stdout

    @buck_test()
    async def test_deprecation(self, buck: Buck) -> None:
        await expect_failure(
            buck.targets("deprecation:"),
            stderr_regex="starlark_deprecation_example.*Going away",
        )

    @buck_test()
    @env("BUCK2_HARD_ERROR", "false")
    async def test_deprecation_is_silent(self, buck: Buck) -> None:
        res = await buck.targets("deprecation:")
        assert "starlark_deprecation_example" not in res.stderr
        log = await buck.log("show")
        assert "starlark_deprecation_example" in log.stdout

    @buck_test()
    @env("BUCK2_HARD_ERROR", "false")
    async def test_deprecation_start_must_be_a_day(self, buck: Buck) -> None:
        await expect_failure(
            buck.targets("deprecation_bad_start:"),
            stderr_regex="yesterday",
        )

    @buck_test(
        # windows errors are slightly different, just skip for now
        skip_for_os=["windows"],
    )
    async def test_configured_graph_deps_collapsed_in_errors(self, buck: Buck) -> None:
        out = await expect_failure(
            buck.cquery(
                "//deps_collapsed:top",
                "-v=0",
                "--console=none",
                "-c",
                "build.execution_platforms=root//deps_collapsed:exec_platforms",
            )
        )
        stderr = re.sub("#[a-f0-9]*\\)", "#00000000)", out.stderr)
        golden(output=stderr, rel_path="deps_collapsed/expected.golden.out")

    @buck_test(
        # windows errors are slightly different, just skip for now
        skip_for_os=["windows"],
    )
    async def test_configured_graph_deps_collapsed_in_errors_2(
        self, buck: Buck
    ) -> None:
        out = await expect_failure(
            buck.cquery(
                "//deps_collapsed:top",
                "-v=0",
                "--console=none",
                "-c",
                "build.execution_platforms=root//deps_collapsed:exec_platforms",
                "-c",
                "core_test_errors.broken_select_in_toolchain=1",
            )
        )
        stderr = re.sub("#[a-f0-9]*\\)", "#00000000)", out.stderr)
        golden(output=stderr, rel_path="deps_collapsed/expected_2.golden.out")
