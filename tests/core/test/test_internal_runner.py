#!/usr/bin/env fbpython
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
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env
from buck2.tests.e2e_util.helper.utils import filter_events, read_what_ran

# Empty test executor forces internal test executor to be used.
INTERNAL_TEST_EXECUTOR = ""


async def _test_runs(buck: Buck) -> list[list[str]]:
    return [
        entry["extra"]["testcases"]
        for entry in await read_what_ran(buck)
        if entry["reason"] == "test.run"
    ]


class InternalRunnerTest(BuckTestCase):
    @buck_test()
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_internal_test_executor(self, buck: Buck) -> None:
        await buck.test(
            ":trivial_pass",
            test_executor=INTERNAL_TEST_EXECUTOR,
        )

    @buck_test()
    @env("TEST_VAR", "BAD_VALUE")
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_internal_test_executor_env(self, buck: Buck) -> None:
        await buck.test(
            ":check_env",
            "--",
            "--env",
            "TEST_VAR=TEST_VALUE",
            test_executor=INTERNAL_TEST_EXECUTOR,
        )

    @buck_test()
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_internal_test_executor_timeout(self, buck: Buck) -> None:
        await expect_failure(
            buck.test(
                ":timeout",
                "--",
                "--timeout",
                "1",
                test_executor=INTERNAL_TEST_EXECUTOR,
            ),
            stderr_regex="Timeout: ",
        )

    @buck_test()
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_listing_preset_skip_is_reported_without_running(
        self, buck: Buck
    ) -> None:
        result = await buck.test(
            ":preset_skip",
            test_executor=INTERNAL_TEST_EXECUTOR,
        )
        assert "Pass 1" in result.stderr, result.stderr
        assert "Skip 1" in result.stderr, result.stderr

        discovered = await filter_events(
            buck, "Event", "data", "Instant", "data", "TestDiscovery", "data", "Tests"
        )
        assert [d["test_names"] for d in discovered] == [["run_me", "skip_me"]]

        skipped = [
            r
            for r in await filter_events(
                buck, "Event", "data", "Instant", "data", "TestResult"
            )
            if r["name"] == "skip_me"
        ]
        assert len(skipped) == 1, skipped
        assert skipped[0]["msg"]["msg"] == "skipped by listing"

        assert await _test_runs(buck) == [["run_me"]]

    @buck_test()
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_listing_preset_skip_for_every_test_runs_nothing(
        self, buck: Buck
    ) -> None:
        result = await buck.test(
            ":all_skipped",
            test_executor=INTERNAL_TEST_EXECUTOR,
        )
        assert "Skip 2" in result.stderr, result.stderr

        assert await _test_runs(buck) == []
