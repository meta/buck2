# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import asyncio

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import ExitCodeV2
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env


class ExitCodeTest(BuckTestCase):
    @buck_test()
    async def test_exit_code_build_success(self, buck: Buck) -> None:
        result = await buck.build(":build_success")
        assert result.process.returncode == ExitCodeV2.SUCCESS.value

    @buck_test()
    async def test_exit_code_build_fail(self, buck: Buck) -> None:
        # TODO(nga): check non-existing command
        # TODO(nga): check local vs RE
        await expect_failure(buck.build(":build_fail"), exit_code=ExitCodeV2.USER_ERROR)

    # Deliberately cause a daemon connection failure.
    @buck_test(allow_daemon_panics=True)
    @env("BUCK2_TEST_FAIL_BUCKD_AUTH", "true")
    # This test case spawns a loose daemon that we can't connect to. On windows
    # this loose daemon will keep holding onto buck-out files after test case finishes
    # and prevent other processes from changing them, so set a termination timeout
    # of 20 seconds so that this loose daemon gets killed before test case finishes.
    @env("BUCK2_TERMINATE_AFTER", "15")
    async def test_exit_code_fail_buckd_auth_for_unknown_reason(
        self, buck: Buck
    ) -> None:
        await expect_failure(
            buck.build(":build_success"), exit_code=ExitCodeV2.CONNECT_ERROR
        )
        await asyncio.sleep(
            20
        )  # Makes sure the daemon terminates before test case finishes
