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
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env


class LocalResourcesTest(BuckTestCase):
    @buck_test()
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_platform_resolution(self, buck: Buck) -> None:
        await buck.test(
            ":my_test",
            test_executor="",
        )
        res = await buck.log("what-ran")
        assert "MY_RESOURCE_ID=42" in res.stdout

    @buck_test(skip_for_os=["windows", "darwin"], disable_daemon_cgroup=False)
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_local_resource_broker_survives_cgroup_cleanup(
        self, buck: Buck
    ) -> None:
        await buck.test(
            ":my_daemon_test",
            test_executor="",
        )
        res = await buck.log("what-ran")
        assert "BROKER_PID=" in res.stdout

    @buck_test()
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_two_resource_types_from_one_broker(self, buck: Buck) -> None:
        # Two resource types served by the same target: one setup, one resource shared by both.
        await buck.test(":two_types_one_broker", test_executor="")
        res = await buck.log("what-ran")
        assert res.stdout.count("test.local_resource_setup") == 1

    @buck_test(skip_for_os=["windows", "darwin"])
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_registered_resource_release_and_failure_compensation(
        self, buck: Buck
    ) -> None:
        events = buck.cwd / "registration-events"
        for mode in (
            "success",
            "failure",
            "bad_descriptor",
            "ack_loss",
            "timeout",
            "withheld",
        ):
            events.unlink(missing_ok=True)
            command = buck.test(
                ":registered_" + mode,
                "-c",
                "buck2.local_resource_cleanup_registration=true",
                test_executor="",
            )
            if mode == "success":
                await command
            else:
                await expect_failure(command)

            async def wait_for_cleanup() -> None:
                while not events.exists() or "cleanup" not in events.read_text():
                    await asyncio.sleep(0.02)

            await asyncio.wait_for(wait_for_cleanup(), timeout=10)
            records = events.read_text().splitlines()
            assert records[0] == "connected" and records[-1] == "cleanup", records
            assert ("registered" in records) == (mode == "success"), records
            assert ("register" in records) == (
                mode in ("success", "ack_loss", "withheld")
            ), records

    @buck_test(skip_for_os=["windows", "darwin"])
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_pending_ack_keeps_other_setups_running(self, buck: Buck) -> None:
        await buck.test(
            ":concurrent_registered",
            "-c",
            "buck2.local_resource_cleanup_registration=true",
            test_executor="",
        )
        events = (buck.cwd / "registration-events").read_text().splitlines()
        assert (
            events.index("register_ack_hold")
            < events.index("registered_wait_for_holder")
            < events.index("registered_ack_hold")
        ), events

    @buck_test(skip_for_os=["windows", "darwin"])
    @env("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")
    async def test_subtarget_aliases_share_one_registered_pool(
        self, buck: Buck
    ) -> None:
        await buck.test(
            ":registered_aliases",
            "-c",
            "buck2.local_resource_cleanup_registration=true",
            test_executor="",
        )
        event_file = buck.cwd / "registration-events"

        async def wait_for_cleanup() -> None:
            while not event_file.exists() or "cleanup" not in event_file.read_text():
                await asyncio.sleep(0.02)

        await asyncio.wait_for(wait_for_cleanup(), timeout=10)
        events = event_file.read_text().splitlines()
        assert events.count("connected") == 1, events
        assert events.count("registered") == 1, events
        assert events.count("cleanup") == 1, events
