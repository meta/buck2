# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import asyncio
import fcntl
import json
import os
from collections.abc import Callable
from uuid import uuid4

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env
from buck2.tests.e2e_util.helper.utils import daemon_is_alive


async def _wait_until(predicate: Callable[[], bool]) -> None:
    async with asyncio.timeout(5):
        while not predicate():
            await asyncio.sleep(0.01)


async def _stop_process(process: asyncio.subprocess.Process | None) -> None:
    if process is not None:
        if process.returncode is None:
            process.kill()
        await asyncio.wait_for(process.communicate(), timeout=10)


class TenantKillStartupTest(BuckTestCase):
    @buck_test(data_dir="test_daemon_data")
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_shared_kill_rechecks_owner_after_waiting_for_startup(
        self, buck: Buck
    ) -> None:
        buck.set_isolation_prefix("kill-race")
        status = json.loads((await buck.server("--status")).stdout)
        startup_config = status["daemon_constraints"]["daemon_startup_config"]
        old_pid = status["process_info"]["pid"]
        daemon_dir = await buck.get_daemon_dir()
        await buck.kill()
        await _wait_until(lambda: not daemon_is_alive(old_pid))
        (daemon_dir / "buckd.info").unlink(missing_ok=True)
        (daemon_dir / "buckd.pid").unlink(missing_ok=True)

        pending_kill = replacement = None
        try:
            with (daemon_dir / "buckd.lifecycle").open("a+b") as lifecycle:
                fcntl.flock(lifecycle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                os.utime(lifecycle.fileno(), ns=(1, 1))
                pending_kill = await buck.kill().start()
                # File::create updates mtime before the client attempts the lock.
                await _wait_until(
                    lambda: (daemon_dir / "buckd.lifecycle").stat().st_mtime_ns != 1
                )
                assert pending_kill.returncode is None
                replacement = await buck.run_buck_command(
                    "daemon",
                    "--dont-daemonize",
                    "--daemon-id",
                    str(uuid4()),
                    startup_config,
                ).start()
                await _wait_until(lambda: (daemon_dir / "buckd.info").exists())
                sibling = Buck(
                    buck.path_to_executable,
                    encoding=buck.encoding,
                    env=dict(buck._env),
                    cwd=buck.cwd,
                    isolation_prefix="sibling",
                )
                replacement_status = json.loads(
                    (await sibling.server("--status")).stdout
                )
                replacement_pid = replacement_status["process_info"]["pid"]
                assert replacement_pid == replacement.pid
                assert replacement_pid != old_pid
                await sibling.targets("//:")
                tenants = json.loads((await sibling.debug("tenants", "--json")).stdout)
                assert {tenant["isolation"] for tenant in tenants} == {
                    "kill-race",
                    "sibling",
                }
                assert pending_kill.returncode is None

            _, stderr = await asyncio.wait_for(pending_kill.communicate(), timeout=15)
            assert pending_kill.returncode != 0, stderr.decode()
            assert b"Error locking buckd.lifecycle" not in stderr, stderr.decode()
            assert replacement.returncode is None
            assert daemon_is_alive(replacement_pid)
            assert (
                json.loads((await sibling.status()).stdout)["process_info"]["pid"]
                == replacement_pid
            )
            tenants = json.loads((await sibling.debug("tenants", "--json")).stdout)
            assert {tenant["isolation"] for tenant in tenants} == {
                "kill-race",
                "sibling",
            }
        finally:
            await _stop_process(pending_kill)
            await _stop_process(replacement)
