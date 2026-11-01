# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import contextlib
import os
import stat
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckException
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env


def _tenants(buck: Buck) -> list[Buck]:
    return [
        Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix=isolation,
        )
        for isolation in ["tenant-one", "tenant-two"]
    ]


async def _kill_tenants(tenants: list[Buck]) -> None:
    for tenant in tenants:
        with contextlib.suppress(BuckException):
            await tenant.kill()


class KillallRepoResilienceTest(BuckTestCase):
    @buck_test(data_dir="test_daemon_data", skip_for_os=["windows"])
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_killall_repo_reports_unreadable_daemon_root(
        self, buck: Buck
    ) -> None:
        if os.geteuid() == 0:
            self.skipTest("unreadable-root killall assertions require a non-root user")
        tenants = _tenants(buck)
        project_dir: Path | None = None
        original_mode: int | None = None
        try:
            for tenant in tenants:
                await tenant.targets("//:rule")
            project_dir = (await tenants[0].get_daemon_dir()).parent
            original_mode = stat.S_IMODE(project_dir.stat().st_mode)
            os.chmod(project_dir, 0o000)
            try:
                failure = await expect_failure(
                    buck._run_buck_command(
                        "killall", "--repo", input=None, rel_cwd=None, env=None
                    ),
                    stderr_regex="Repository tenant eviction failed",
                )
            finally:
                os.chmod(project_dir, original_mode)
            assert "Evicted 0 tenant(s)" in failure.stderr
            assert "failed for 1 daemon(s)" in failure.stderr
            assert "Failed to inspect daemon directory" in failure.stderr
            for tenant in tenants:
                assert "no buckd running" not in (await tenant.status()).stderr
                await tenant.targets("//:rule")
        finally:
            if project_dir is not None and original_mode is not None:
                os.chmod(project_dir, original_mode)
            await _kill_tenants(tenants)
