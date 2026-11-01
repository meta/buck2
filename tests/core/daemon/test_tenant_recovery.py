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
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckResult
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import (
    buck_test,
    BuckTestCase,
    env,
    parametrize,
)
from buck2.tests.e2e_util.helper.utils import daemon_is_alive


async def _wait_for_exit(pid: int) -> None:
    async with asyncio.timeout(10):
        while daemon_is_alive(pid):
            await asyncio.sleep(0.01)


async def _invoke(buck: Buck, command: str) -> BuckResult:
    args = {
        "targets": ("targets", "//:rule"),
        "kill": ("kill",),
        "clean": ("clean",),
        "killall": ("killall", "--repo"),
    }
    return await buck.run_buck_command(*args[command])


def _damage_metadata(daemon_dir: Path, metadata: str) -> None:
    info_path = daemon_dir / "buckd.info"
    if metadata == "missing-info":
        info_path.unlink()
    elif metadata == "malformed-info":
        info_path.write_text("not JSON")
    else:
        info = json.loads(info_path.read_text())
        info["endpoint"] = "tcp:127.0.0.1:0"
        info_path.write_text(json.dumps(info))
        pid_path = daemon_dir / "buckd.pid"
        if metadata == "missing-pid":
            pid_path.unlink()
        else:
            assert metadata == "mismatched-pid"
            pid_path.write_text(str(info["pid"] + 1))


class TenantRecoveryTest(BuckTestCase):
    @buck_test(data_dir="test_daemon_data")
    @env("BUCK2_SHARE_DAEMON", "true")
    @env("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "120")
    @parametrize("command", ["targets", "kill", "clean", "killall"])
    @parametrize("alive", [False, True])
    @parametrize(
        "metadata", ["missing-info", "malformed-info", "missing-pid", "mismatched-pid"]
    )
    async def test_shared_recovery_refuses_uncertain_metadata(
        self, buck: Buck, command: str, alive: bool, metadata: str
    ) -> None:
        await buck.targets("//:rule")
        daemon_dir = await buck.get_daemon_dir()
        info_path = daemon_dir / "buckd.info"
        pid_path = daemon_dir / "buckd.pid"
        original_info = info_path.read_text()
        original_pid = pid_path.read_text()
        pid = json.loads(original_info)["pid"]
        cache = buck.cwd / "buck-out" / (buck.isolation_prefix or "v2") / "cache"
        assert cache.is_dir()
        if not alive:
            await buck.kill()
            await _wait_for_exit(pid)
        try:
            _damage_metadata(daemon_dir, metadata)
            damaged_info = info_path.read_bytes() if info_path.exists() else None
            damaged_pid = pid_path.read_bytes() if pid_path.exists() else None
            await expect_failure(_invoke(buck, command))
            assert cache.is_dir()
            assert daemon_is_alive(pid) == alive
            assert (
                info_path.read_bytes() if info_path.exists() else None
            ) == damaged_info
            assert (pid_path.read_bytes() if pid_path.exists() else None) == damaged_pid
        finally:
            info_path.write_text(original_info)
            pid_path.write_text(original_pid)
            await buck.clean()
        assert not cache.exists()
        await buck.targets("//:rule")

    @buck_test(data_dir="test_daemon_data")
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("command", ["targets", "kill", "clean", "killall"])
    async def test_shared_recovery_accepts_consistent_exited_metadata(
        self, buck: Buck, command: str
    ) -> None:
        await buck.targets("//:rule")
        pid = json.loads((await buck.status()).stdout)["process_info"]["pid"]
        await buck.kill()
        await _wait_for_exit(pid)
        await _invoke(buck, command)
        await buck.targets("//:rule")
        assert json.loads((await buck.status()).stdout)["process_info"]["pid"] != pid

    @buck_test(data_dir="test_daemon_data")
    @env("BUCK2_SHARE_DAEMON", "true")
    @env("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "120")
    async def test_killall_repo_surfaces_inactivity_guidance(self, buck: Buck) -> None:
        daemon_dir: Path | None = None
        original_info: str | None = None
        try:
            await buck.targets("//:rule")
            daemon_dir = await buck.get_daemon_dir()
            info_path = daemon_dir / "buckd.info"
            pid_path = daemon_dir / "buckd.pid"
            original_info = info_path.read_text()
            original_pid = pid_path.read_bytes()
            pid = json.loads(original_info)["pid"]
            _damage_metadata(daemon_dir, "malformed-info")
            failure = await expect_failure(
                _invoke(buck, "killall"),
                stderr_regex="Cannot verify shared daemon inactivity",
            )
            assert (
                "restore consistent buckd.info and buckd.pid before retrying"
                in failure.stderr
            )
            assert pid_path.read_bytes() == original_pid
            assert daemon_is_alive(pid)
        finally:
            if daemon_dir is not None and original_info is not None:
                (daemon_dir / "buckd.info").write_text(original_info)
            await buck.clean()
