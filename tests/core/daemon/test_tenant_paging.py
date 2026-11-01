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
import sqlite3
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import (
    buck_test,
    BuckTestCase,
    env,
    parametrize,
)
from buck2.tests.e2e_util.helper.utils import daemon_is_alive


async def _tenant_value(buck: Buck, config: Path) -> str:
    result = await buck.build(
        "//:tenant_value", "--config-file", str(config), "--build-report=-"
    )
    return result.get_build_report().output_for_target("//:tenant_value").read_text()


async def _page_out(buck: Buck, *, require_new_values: bool = True) -> int:
    out = (await buck.debug("hydration", "page-out")).stdout
    try:
        summary = json.loads(out)
    except json.JSONDecodeError as e:
        raise AssertionError(f"page-out did not print a JSON summary:\n{out}") from e
    assert summary.get("error") is None, f"page-out failed:\n{out}"
    data_keys_out = summary.get("data_keys_out", 0)
    if require_new_values:
        assert data_keys_out > 0, f"expected positive paged values:\n{out}"
    assert summary["daemon_data_keys_out"] > 0, out
    return summary["daemon_data_keys_in"]


def _materializer_identity(buck: Buck) -> str | None:
    db = (
        buck.cwd
        / "buck-out"
        / (buck.isolation_prefix or "v2")
        / "cache/materializer_state/db.sqlite"
    )
    if not db.is_file():
        return None
    with sqlite3.connect(db) as connection:
        row = connection.execute(
            "SELECT value FROM created_by WHERE key = 'timestamp_on_initialization'"
        ).fetchone()
        return row[0] if row else None


def _tenant_dice_store(buck: Buck) -> Path:
    return buck.cwd / "buck-out" / (buck.isolation_prefix or "v2") / "cache/dice_state"


def _daemon_shared_dice_dirs(buck: Buck) -> list[Path]:
    parent = buck.cwd / "buck-out" / "._buck2" / "dice_state"
    if not parent.is_dir():
        return []
    return sorted(p for p in parent.iterdir() if p.is_dir())


async def _pid(buck: Buck) -> int:
    return json.loads((await buck.status()).stdout)["process_info"]["pid"]


async def _assert_daemon_exits(pid: int) -> None:
    for _ in range(200):
        if not daemon_is_alive(pid):
            return
        await asyncio.sleep(0.05)
    assert not daemon_is_alive(pid), "Daemon survived final tenant eviction"


class TenantPagingTest(BuckTestCase):
    # With a shared engine, page-in critical path entries can report time spans
    # that overlap their neighbours (`critical_path_entry_overlap`). This test
    # checks paging storage lifetime, not critical path accounting.
    @buck_test(
        data_dir="test_daemon_data",
        skip_for_os=["windows"],
        allow_soft_errors=True,
    )
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("share_dice", [False, True])
    async def test_tenant_paging_storage_lifetime(
        self, buck: Buck, tmp_path: Path, share_dice: bool
    ) -> None:
        startup_config = buck.cwd / ".buckconfig"
        startup_config.write_text(
            startup_config.read_text()
            + f"\n[buck2]\nshare_dice = {str(share_dice).lower()}\n"
        )
        (buck.get_settings_home_dir() / ".bucksettings.local.toml").write_text(
            "[hydration]\n"
            "enable_paging = true\n"
            "page_out_on_idle = false\n"
            'page_out_policy = "once_per_key"\n'
            'eviction_policy = "on_idle"\n'
        )
        initial_config = tmp_path / "initial.bcfg"
        initial_config.write_text("[tenant]\nvalue = initial\n")
        sibling_config = tmp_path / "sibling.bcfg"
        sibling_config.write_text("[tenant]\nvalue = sibling\n")
        other_config = tmp_path / "other.bcfg"
        other_config.write_text("[tenant]\nvalue = other\n")
        tenant = buck
        sibling = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="survivor",
        )
        other = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env={**buck._env, "BUCK2_SHARE_DAEMON": "false"},
            cwd=buck.cwd,
            isolation_prefix="other",
        )
        try:
            assert await _tenant_value(tenant, initial_config) == "initial"
            assert await _tenant_value(sibling, sibling_config) == "sibling"
            pid = await _pid(sibling)
            assert await _pid(tenant) == pid

            initial_keys_in = await _page_out(tenant)
            identity = _materializer_identity(tenant)
            assert identity is not None

            if share_dice:
                shared_dirs = _daemon_shared_dice_dirs(buck)
                assert len(shared_dirs) == 1
                paging_path = shared_dirs[0]
                assert any(paging_path.iterdir())
            else:
                assert _daemon_shared_dice_dirs(buck) == []
                paging_path = _tenant_dice_store(tenant)
                assert any(paging_path.iterdir())

            await other.build("//:tenant_value", "--config-file", str(other_config))
            await _page_out(other)
            other_pid = await _pid(other)
            assert other_pid != pid
            other_dirs = (
                [p for p in _daemon_shared_dice_dirs(buck) if p != paging_path]
                if share_dice
                else []
            )
            if share_dice:
                assert len(other_dirs) == 1

            await tenant.kill()
            assert await _pid(sibling) == pid
            assert await _tenant_value(tenant, initial_config) == "initial"
            assert _materializer_identity(tenant) == identity
            if share_dice:
                recreated_keys_in = await _page_out(tenant, require_new_values=False)
                assert recreated_keys_in > initial_keys_in
                assert paging_path in _daemon_shared_dice_dirs(buck)
                assert any(paging_path.iterdir())

            await tenant.kill()
            await tenant.clean()
            if share_dice:
                assert paging_path in _daemon_shared_dice_dirs(buck)
                assert any(paging_path.iterdir())
            else:
                assert not _tenant_dice_store(tenant).exists()
            assert await _tenant_value(tenant, initial_config) == "initial"
            new_identity = _materializer_identity(tenant)
            assert new_identity is not None
            assert new_identity != identity
            clean_keys_in = await _page_out(tenant, require_new_values=not share_dice)
            assert await _tenant_value(tenant, initial_config) == "initial"
            replayed_keys_in = await _page_out(tenant, require_new_values=False)
            if not share_dice:
                assert replayed_keys_in > clean_keys_in
            assert await _tenant_value(sibling, sibling_config) == "sibling"
            await _page_out(sibling, require_new_values=not share_dice)
            assert await _pid(tenant) == pid

            await tenant.kill()
            await sibling.kill()
            await _assert_daemon_exits(pid)
            if share_dice:
                assert await _tenant_value(tenant, initial_config) == "initial"
                assert paging_path not in _daemon_shared_dice_dirs(buck)
                for p in other_dirs:
                    assert p.is_dir()
            await other.kill()
            await _assert_daemon_exits(other_pid)
            if share_dice:
                await tenant.kill()
                assert await _tenant_value(tenant, initial_config) == "initial"
                for p in other_dirs:
                    assert not p.exists()
        finally:
            await tenant.kill()
            await sibling.kill()
            await other.kill()
