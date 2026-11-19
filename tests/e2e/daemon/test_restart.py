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


def restart_error(reason: str) -> str:
    """The error `buck2 debug restart` provides, which the interrupted commands fail with."""
    return f"Daemon restart requested: {reason}"


async def daemon_id(buck: Buck) -> str:
    status = json.loads((await buck.status()).stdout)
    return status["daemon_constraints"]["daemon_id"]


class Gate:
    """A directory that the action of a gated command waits in. The first attempt at a tag
    marks itself started and waits to be released. Later attempts finish at once."""

    def __init__(self, directory: Path) -> None:
        self.directory = directory

    def release(self) -> None:
        (self.directory / "released").touch()

    async def wait_started(self, tag: str) -> None:
        async with asyncio.timeout(TIMEOUT):
            while not (self.directory / tag).exists():
                await asyncio.sleep(0.05)


def gated_command(
    buck: Buck, gate: Gate, tag: str, *, streaming: bool = False
) -> Process[BuckResult, BuckException]:
    """A command that prints `done:<tag>` to stdout once its action has passed the gate. With
    `streaming` it prints `prefix:<tag>` first, before it runs the action."""
    label = "streaming" if streaming else "gated"
    python = shutil.which("fbpython") or sys.executable
    return buck.bxl(
        f"root//restart.bxl:{label}",
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
        self.raw: list[asyncio.subprocess.Process] = []

    def start(self, tag: str) -> asyncio.Future[BuckResult]:
        future = asyncio.ensure_future(gated_command(self.buck, self.gate, tag))
        self.futures.append(future)
        return future

    async def close(self) -> None:
        self.gate.release()
        for process in self.raw:
            if process.returncode is None:
                process.kill()
            await process.wait()
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
async def commands(buck: Buck) -> AsyncIterator[RunningCommands]:
    with tempfile.TemporaryDirectory() as directory:
        running = RunningCommands(buck, Gate(Path(directory)))
        try:
            yield running
        finally:
            await running.close()


class CoordinatedRestartTest(TestCase):
    async def boot(self, buck: Buck) -> str:
        """Starts the daemon, and returns its id."""
        await buck.targets("//:ok")
        return await daemon_id(buck)

    async def request_restart(self, buck: Buck, reason: str) -> None:
        await asyncio.wait_for(
            admin_client(buck).debug("restart", f"--reason={reason}"), TIMEOUT
        )

    def assert_retried(
        self, buck: Buck, result: BuckResult, tag: str, reason: str
    ) -> None:
        self.assertEqual(result.stdout.count(f"done:{tag}"), 1)
        self.assertEqual(result.stderr.count(RETRY_NOTICE), 1)
        record = result.invocation_record()
        self.assertIsNotNone(record["restarted_trace_id"])
        self.assertNotEqual(record["trace_id"], record["restarted_trace_id"])
        # Only the first attempt asks for a restart.
        self.assertFalse(record["should_restart"])
        # The retry overwrote the first attempt's invocation record. Its command report holds
        # the same errors: the one the command that requested the restart provided.
        logs = buck.cwd / "buck-out" / (buck.isolation_prefix or "v2") / "log"
        report = json.loads(
            (logs / record["restarted_trace_id"] / "command_report.json").read_text()
        )
        self.assertEqual(len(report["error_messages"]), 1)
        self.assertIn(restart_error(reason), report["error_messages"][0])

    def assert_failed_with_restart_error(
        self, error: BuckException, reason: str
    ) -> None:
        """A command the restart interrupted, and that was not retried, reports the same
        invocation record as the first attempt of one that was."""
        self.assertNotIn(RETRY_NOTICE, error.stderr)
        record = error.invocation_record()
        self.assertIsNone(record["restarted_trace_id"])
        reported = record.single_error()
        self.assertIn(restart_error(reason), reported["message"])
        self.assertIn("INTERRUPTED_BY_DAEMON_SHUTDOWN", reported["tags"])

    @buck_test(inplace=False, write_invocation_record=True, allow_soft_errors=True)
    async def test_interrupted_commands_retry_on_one_replacement(
        self, buck: Buck
    ) -> None:
        old = await self.boot(buck)
        tags = ["client-a", "client-b", "client-c"]
        async with commands(buck) as running:
            pending = [running.start(tag) for tag in tags]
            await asyncio.gather(*(running.gate.wait_started(tag) for tag in tags))
            await self.request_restart(buck, "concurrent-clients")
            results = await asyncio.wait_for(asyncio.gather(*pending), TIMEOUT)
            for tag, result in zip(tags, results):
                self.assert_retried(buck, result, tag, "concurrent-clients")
            replacement = await daemon_id(buck)
            self.assertNotEqual(replacement, old)
            # A command that the restart did not interrupt uses the same replacement.
            await buck.targets("//:ok")
            self.assertEqual(await daemon_id(buck), replacement)

    @buck_test(inplace=False, write_invocation_record=True, allow_soft_errors=True)
    async def test_stdout_already_delivered_refuses_retry(self, buck: Buck) -> None:
        await self.boot(buck)
        async with commands(buck) as running:
            request = gated_command(buck, running.gate, "stdout", streaming=True)
            process = await request.start()
            running.raw.append(process)
            assert process.stdout is not None
            prefix = await asyncio.wait_for(process.stdout.readline(), TIMEOUT)
            self.assertEqual(prefix, b"prefix:stdout\n")
            # start() consumed the creation coroutine; finish the same raw process,
            # retaining the harness's result/exception and invocation-record handling.
            pending = asyncio.ensure_future(
                request._get_result_or_raise_exception(process)
            )
            running.futures.append(pending)
            await running.gate.wait_started("stdout")
            await self.request_restart(buck, "stdout-not-replayable")
            error = await expect_failure(asyncio.wait_for(pending, TIMEOUT))
            self.assertNotIn("done:stdout", error.stdout)
            self.assert_failed_with_restart_error(error, "stdout-not-replayable")
            # The client asked to restart, as for any restart, and then could not.
            self.assertTrue(error.invocation_record()["should_restart"])

    @buck_test(inplace=False, write_invocation_record=True, allow_soft_errors=True)
    async def test_restarter_off_does_not_retry(self, buck: Buck) -> None:
        (buck.cwd / ".buckconfig.local").write_text("[buck2]\n  restarter = false\n")
        await self.boot(buck)
        async with commands(buck) as running:
            pending = running.start("restarter-off")
            await running.gate.wait_started("restarter-off")
            await self.request_restart(buck, "restarter-off")
            error = await expect_failure(asyncio.wait_for(pending, TIMEOUT))
            self.assert_failed_with_restart_error(error, "restarter-off")
            self.assertFalse(error.invocation_record()["should_restart"])

    @buck_test(inplace=False, write_invocation_record=True)
    async def test_ordinary_kill_does_not_retry(self, buck: Buck) -> None:
        await self.boot(buck)
        async with commands(buck) as running:
            pending = running.start("ordinary-kill")
            await running.gate.wait_started("ordinary-kill")
            await admin_client(buck).kill()
            error = await expect_failure(asyncio.wait_for(pending, TIMEOUT))
            self.assertNotIn(RETRY_NOTICE, error.stderr)
