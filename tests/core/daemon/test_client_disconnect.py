# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import asyncio
import json

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env
from buck2.tests.e2e_util.helper.utils import daemon_is_alive

# Generous, so the client has exited before the stall ends even on a slow CI machine.
STALL_MS = 2000


async def _daemon_pid(buck: Buck) -> int:
    return json.loads((await buck.server("--status")).stdout)["process_info"]["pid"]


async def _wait_for_active_command(
    buck: Buck, client: asyncio.subprocess.Process
) -> None:
    for _ in range(600):
        if json.loads((await buck.status()).stdout)["active_commands"]:
            return
        if client.returncode is not None:
            _, stderr = await client.communicate()
            raise AssertionError(
                f"The client exited before its command was registered:\n{stderr.decode()}"
            )
        await asyncio.sleep(0.1)
    raise AssertionError("The command never showed up in active_commands")


class ClientDisconnectTest(BuckTestCase):
    """
    The daemon must survive its clients going away. The fixture fails any test whose daemon
    panicked, so these only check that the same daemon is still running.
    """

    @buck_test(data_dir="test_daemon_data")
    @env("BUCK2_TEST_COMMAND_END_STALL_MS", str(STALL_MS))
    async def test_client_leaving_with_its_result(self, buck: Buck) -> None:
        """
        The client exits as soon as it has its result, which cancels the command's task on
        the daemon while that task may still be finishing. The daemon once mistook that for a
        cancelled command and crashed (D129074969). The stall makes the test hit that window
        every time.
        """
        await buck.targets("//:rule")
        pid = await _daemon_pid(buck)

        await buck.targets("//:rule")
        await asyncio.sleep(STALL_MS / 1000 + 1)

        assert daemon_is_alive(pid), (
            "The daemon died after the client left with its result"
        )
        assert await _daemon_pid(buck) == pid

    @buck_test(data_dir="test_daemon_data")
    async def test_client_killed_while_its_command_runs(self, buck: Buck) -> None:
        await buck.targets("//:rule")
        pid = await _daemon_pid(buck)

        client = await buck.build(
            "//:long_running", "--local-only", "--no-remote-cache"
        ).start()
        try:
            await _wait_for_active_command(buck, client)
        finally:
            if client.returncode is None:
                client.kill()
                await client.communicate()

        for _ in range(100):
            if not json.loads((await buck.status()).stdout)["active_commands"]:
                break
            await asyncio.sleep(0.1)
        else:
            raise AssertionError("The killed client's command was not cancelled")

        assert daemon_is_alive(pid), "The daemon died when its client was killed"
        assert await _daemon_pid(buck) == pid
        await buck.targets("//:rule")
