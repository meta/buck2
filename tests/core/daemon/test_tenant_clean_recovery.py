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
import sys
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import (
    buck_test,
    BuckTestCase,
    env,
    parametrize,
)
from buck2.tests.e2e_util.helper.utils import daemon_is_alive


async def _wait_for_exit(pid: int) -> None:
    for _ in range(100):
        if not daemon_is_alive(pid):
            return
        await asyncio.sleep(0.1)
    raise AssertionError(f"daemon pid {pid} is still alive")


async def _exited_process_pid() -> int:
    process = await asyncio.create_subprocess_exec(sys.executable, "-c", "pass")
    assert await process.wait() == 0
    assert not daemon_is_alive(process.pid)
    return process.pid


def _tenant_cache(buck: Buck) -> Path:
    return buck.cwd / "buck-out" / (buck.isolation_prefix or "v2") / "cache"


class TenantCleanRecoveryTest(BuckTestCase):
    @buck_test(data_dir="test_daemon_data")
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("metadata", ["missing", "not-json"])
    async def test_shared_clean_requires_repair_dead_daemon_corrupt_info(
        self, buck: Buck, metadata: str
    ) -> None:
        await buck.targets("//:rule")
        assert _tenant_cache(buck).is_dir()
        daemon_dir = await buck.get_daemon_dir()
        info_path = daemon_dir / "buckd.info"
        original_info = info_path.read_text()
        pid = json.loads(original_info)["pid"]
        await buck.kill()
        await _wait_for_exit(pid)
        try:
            if metadata == "missing":
                info_path.unlink()
            else:
                info_path.write_text("not JSON")
            damaged_info = info_path.read_bytes() if info_path.exists() else None
            await expect_failure(buck.clean())
            assert _tenant_cache(buck).is_dir()
            assert not daemon_is_alive(pid)
            assert (
                info_path.read_bytes() if info_path.exists() else None
            ) == damaged_info
        finally:
            info_path.write_text(original_info)
        await buck.clean()
        assert not _tenant_cache(buck).exists()
        await buck.targets("//:rule")
        assert _tenant_cache(buck).is_dir()

    @buck_test(data_dir="test_daemon_data")
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("metadata", ["missing", "not-json", "unreachable-dead-pid"])
    @parametrize("pid_file", ["unchanged", "exited"])
    async def test_shared_clean_refuses_live_daemon_corrupt_info(
        self, buck: Buck, metadata: str, pid_file: str
    ) -> None:
        await buck.targets("//:rule")
        assert _tenant_cache(buck).is_dir()
        daemon_dir = await buck.get_daemon_dir()
        info_path = daemon_dir / "buckd.info"
        pid_path = daemon_dir / "buckd.pid"
        original_info = info_path.read_text()
        original_pid = pid_path.read_text()
        pid = json.loads(original_info)["pid"]
        try:
            if metadata == "missing":
                info_path.unlink()
            elif metadata == "not-json":
                info_path.write_text("not JSON")
            else:
                info = json.loads(original_info)
                info["pid"] = await _exited_process_pid()
                info["endpoint"] = "tcp:127.0.0.1:0"
                info_path.write_text(json.dumps(info))
            if pid_file == "exited":
                pid_path.write_text(str(await _exited_process_pid()))
            await expect_failure(buck.clean())
            assert daemon_is_alive(pid)
            assert _tenant_cache(buck).is_dir()
        finally:
            info_path.write_text(original_info)
            pid_path.write_text(original_pid)
            await buck.kill()
