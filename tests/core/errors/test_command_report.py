# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import asyncio
import json
from pathlib import Path
from typing import Any, Callable

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import ExitCodeV2
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env
from buck2.tests.e2e_util.helper.golden import golden


def command_report_test(name: str, command: list[str]) -> Callable[..., Any]:
    async def impl(self: BuckTestCase, buck: Buck, tmp_path: Path) -> None:
        report = tmp_path / "command_report.json"
        await expect_failure(buck.build("--command-report-path", str(report), *command))

        with open(report) as f:
            report = json.loads(f.read())
        del report["trace_id"]

        golden(
            output=json.dumps(report, indent=2, sort_keys=True),
            rel_path="fixtures/" + name + ".golden.json",
        )
        pass

    return buck_test()(impl)


class CommandReportTest(BuckTestCase):
    # Test build with a couple of build errors
    test_command_report_build_errors = command_report_test(
        "test_command_report_build_errors", [":fail1", ":fail2"]
    )

    # Set Watchman timeout to 0 to mimic a Watchman Timeout error.
    @buck_test(extra_buck_config={"buck2": {"file_watcher": "watchman"}})
    @env("BUCK2_WATCHMAN_TIMEOUT", "0")
    async def test_command_report_watchman_error(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        report = tmp_path / "command_report.json"
        await expect_failure(
            buck.build("--command-report-path", str(report), ":build_success")
        )

        with open(report) as f:
            report = json.loads(f.read())

        assert report["exit_code"] == ExitCodeV2.USER_ERROR.value
        assert "SyncableQueryHandler returned an error" in report["error_messages"][0]

    # Early client error that doesn't show up in invocation records
    @buck_test()
    @env("BUCK2_TEST_INIT_DAEMON_ERROR", "true")
    async def test_command_report_init_daemon_error(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        report = tmp_path / "command_report.json"
        await expect_failure(
            buck.build("--command-report-path", str(report), ":build_success")
        )

        with open(report) as f:
            report = json.loads(f.read())

        assert report["exit_code"] == ExitCodeV2.CONNECT_ERROR.value
        assert "Injected init daemon error" in report["error_messages"][0]

    # Deliberately cause a daemon connection failure.
    @buck_test(write_invocation_record=True, allow_daemon_panics=True)
    @env("BUCK2_TEST_FAIL_BUCKD_AUTH", "true")
    # This test case spawns a loose daemon that we can't connect to. On windows
    # this loose daemon will keep holding onto buck-out files after test case finishes
    # and prevent other processes from changing them, so set a termination timeout
    # of 20 seconds so that this loose daemon gets killed before test case finishes.
    @env("BUCK2_TERMINATE_AFTER", "15")
    async def test_exit_result_connection_error(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        report = tmp_path / "command_report.json"
        res = await expect_failure(
            buck.build(
                "--command-report-path",
                str(report),
                ":build_success",
            )
        )

        record = res.invocation_record()
        with open(report) as f:
            report = json.loads(f.read())

        assert report["exit_code"] == ExitCodeV2.CONNECT_ERROR.value
        assert "injected auth error" in report["error_messages"][0]
        assert record["exit_result_name"] == "CONNECT_ERROR"

        await asyncio.sleep(
            20
        )  # Makes sure the daemon terminates before test case finishes

    # Late client error takes precedence over action errors
    @buck_test(write_invocation_record=True)
    @env("BUCK2_TEST_BUILD_ERROR", "true")
    async def test_command_report_post_build_client_error(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        report = tmp_path / "command_report.json"
        # Failed build that should have some action errors
        res = await expect_failure(
            buck.build(
                "--command-report-path",
                str(report),
                ":fail1",
            )
        )

        record = res.invocation_record()
        errors = record["errors"]
        # There's only 1 error message and it's the late client error that's injected
        assert len(errors) == 1
        assert errors[0]["message"] == "Injected Build Response Error"

        with open(report) as f:
            report = json.loads(f.read())

        assert len(report["error_messages"]) == 1
        assert report["exit_code"] == ExitCodeV2.INFRA_ERROR.value
        assert "Injected Build Response Error" in report["error_messages"][0]

        assert record["exit_result_name"] == "INFRA_ERROR"

    @buck_test()
    async def test_cleanup_timeout(self, buck: Buck, tmp_path: Path) -> None:
        report = tmp_path / "command_report.json"
        await buck.targets("--command-report-path", str(report), ":")

        with open(report) as f:
            report = json.loads(f.read())

        # test commands have scribe logging disabled, which is reported as a finalizing error
        finalizing_errors = report["finalizing_error_messages"]
        assert len(finalizing_errors) == 1
        assert "'invocation recorder' failed to finalize" in finalizing_errors[0]
        assert "Scribe sink not enabled" in finalizing_errors[0]

    # Should match behavior of command report test in buck wrapper
    @buck_test(data_dir="empty_buckconfig")
    async def test_empty_buckconfig(self, buck: Buck, tmp_path: Path) -> None:
        uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        await expect_failure(
            buck.targets(
                ":",
                env={"BUCK_WRAPPER_UUID": uuid},
            )
        )
        report_path = buck.cwd / "buck-out/v2/log" / uuid / "command_report.json"

        with open(report_path) as f:
            report = json.loads(f.read())

        assert "Error creating cell resolver" in report["error_messages"][0]
