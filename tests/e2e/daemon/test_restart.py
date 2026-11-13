# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import asyncio
import copy
import json
import shutil
import sys
import tempfile
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckException, BuckResult
from buck2.tests.e2e_util.api.process import Process
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test
from later.unittest import TestCase

TIMEOUT = 120.0
RETRY_NOTICE = "Your command will now restart"


def admin_client(buck: Buck) -> Buck:
    client = copy.copy(buck)
    client.write_invocation_record = False
    return client


async def daemon_id(buck: Buck) -> str:
    status = json.loads((await buck.status()).stdout)
    return status["daemon_constraints"]["daemon_id"]


class Gate:
    """A directory that the action of a gated command waits in. The first attempt at a tag
    marks itself started and waits to be released, and then fails if the gate is
    `fail_first`. Later attempts finish at once."""

    def __init__(self, directory: Path, fail_first: bool = False) -> None:
        self.directory = directory
        if fail_first:
            (directory / "fail").touch()

    def release(self) -> None:
        (self.directory / "released").touch()

    async def wait_started(self, tag: str) -> None:
        async with asyncio.timeout(TIMEOUT):
            while not (self.directory / tag).exists():
                await asyncio.sleep(0.05)


def gated_command(
    buck: Buck, gate: Gate, tag: str
) -> Process[BuckResult, BuckException]:
    """A command whose action waits at the gate. It writes nothing to stdout, which
    would keep its client from running it again."""
    python = shutil.which("fbpython") or sys.executable
    return buck.bxl(
        "root//restart.bxl:gated",
        "--local-only",
        "--no-remote-cache",
        "--",
        "--python",
        python,
        "--tag",
        tag,
        "--gate",
        str(gate.directory),
    )


class RunningCommands:
    def __init__(self, buck: Buck, gate: Gate) -> None:
        self.buck = buck
        self.gate = gate
        self.futures: list[asyncio.Future[BuckResult]] = []

    def start(self, tag: str) -> asyncio.Future[BuckResult]:
        future = asyncio.ensure_future(gated_command(self.buck, self.gate, tag))
        self.futures.append(future)
        return future

    async def close(self) -> None:
        self.gate.release()
        await asyncio.wait_for(admin_client(self.buck).kill(), 30)
        results = await asyncio.wait_for(
            asyncio.gather(*self.futures, return_exceptions=True), 30
        )
        for result in results:
            if isinstance(result, BaseException) and not isinstance(
                result, (BuckException, asyncio.CancelledError)
            ):
                raise result


@asynccontextmanager
async def commands(
    buck: Buck, fail_first: bool = False
) -> AsyncIterator[RunningCommands]:
    with tempfile.TemporaryDirectory() as directory:
        running = RunningCommands(buck, Gate(Path(directory), fail_first))
        try:
            yield running
        finally:
            await running.close()


class DebugRestartTest(TestCase):
    async def boot(self, buck: Buck) -> str:
        """Starts the daemon, and returns its id."""
        await buck.targets("//:ok")
        return await daemon_id(buck)

    async def request_restart(self, buck: Buck, reason: str) -> None:
        await asyncio.wait_for(
            admin_client(buck).debug("restart", f"--reason={reason}"), TIMEOUT
        )

    @buck_test(inplace=False, write_invocation_record=True, allow_soft_errors=True)
    async def test_command_that_fails_after_the_request_restarts_on_a_new_daemon(
        self, buck: Buck
    ) -> None:
        old = await self.boot(buck)
        async with commands(buck, fail_first=True) as running:
            pending = running.start("failing")
            await running.gate.wait_started("failing")
            await self.request_restart(buck, "failing-command")
            # The request only has the daemon report its state. Nothing kills the daemon
            # until a command that failed is run again.
            self.assertEqual(await daemon_id(buck), old)
            running.gate.release()
            result = await asyncio.wait_for(pending, TIMEOUT)
            self.assertEqual(result.stderr.count(RETRY_NOTICE), 1)
            self.assertIsNotNone(result.invocation_record()["restarted_trace_id"])
            self.assertNotEqual(await daemon_id(buck), old)

    @buck_test(inplace=False, write_invocation_record=True, allow_soft_errors=True)
    async def test_command_that_succeeds_after_the_request_is_not_restarted(
        self, buck: Buck
    ) -> None:
        old = await self.boot(buck)
        async with commands(buck) as running:
            pending = running.start("succeeding")
            await running.gate.wait_started("succeeding")
            await self.request_restart(buck, "succeeding-command")
            running.gate.release()
            result = await asyncio.wait_for(pending, TIMEOUT)
            self.assertNotIn(RETRY_NOTICE, result.stderr)
            self.assertEqual(await daemon_id(buck), old)

    @buck_test(inplace=False, write_invocation_record=True, allow_soft_errors=True)
    async def test_restarter_off_does_not_restart(self, buck: Buck) -> None:
        (buck.cwd / ".buckconfig.local").write_text("[buck2]\n  restarter = false\n")
        old = await self.boot(buck)
        async with commands(buck, fail_first=True) as running:
            pending = running.start("restarter-off")
            await running.gate.wait_started("restarter-off")
            await self.request_restart(buck, "restarter-off")
            running.gate.release()
            error = await expect_failure(asyncio.wait_for(pending, TIMEOUT))
            self.assertNotIn(RETRY_NOTICE, error.stderr)
            self.assertFalse(error.invocation_record()["should_restart"])
            self.assertEqual(await daemon_id(buck), old)
