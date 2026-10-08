# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


from __future__ import annotations

import asyncio
import contextlib
import gzip
import json
import os
import platform
import re
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path
from uuid import uuid4

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckException, BuildResult
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import (
    buck_test,
    BuckTestCase,
    env,
    parametrize,
)
from buck2.tests.e2e_util.helper.utils import daemon_is_alive, filter_events


class DaemonTest(BuckTestCase):
    @buck_test()
    @env("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")
    async def test_inactivity_timeout(self, buck: Buck) -> None:
        #######################################################
        # Recommend running this test in opt mode
        # Otherwise the command that is run here
        # could take longer than 1 second to finish
        # causing this test to be flaky
        #######################################################

        # this will start the daemon
        status = await buck.server("--status")
        pid = json.loads(status.stdout)["process_info"]["pid"]
        daemon_dir = await buck.get_daemon_dir()

        time.sleep(1)  # 1 sec timeout

        # check it's dead
        for _ in range(20):
            time.sleep(1)
            if not daemon_is_alive(pid):
                result = await buck.status()
                assert "no buckd running" == result.stderr.splitlines()[-1]

                stderr = (daemon_dir / "buckd.stderr").read_text()
                assert "inactivity timeout elapsed" in stderr
                return

        raise AssertionError(f"Server with pid {pid} did not die in 20 seconds")

    @buck_test()
    async def test_server_endpoint_output(self, buck: Buck) -> None:
        result = await buck.server()
        stdout = result.stdout.strip()
        assert stdout.startswith("buckd.endpoint=")
        assert stdout.removeprefix("buckd.endpoint=")

    @buck_test()
    async def test_server_status_output(self, buck: Buck) -> None:
        result = await buck.server("--status")
        status = json.loads(result.stdout)
        pid = status["process_info"]["pid"]
        assert isinstance(pid, int)
        assert pid > 0

    @buck_test()
    async def test_server_status_snapshot_output(self, buck: Buck) -> None:
        result = await buck.server("--status", "--snapshot")
        status = json.loads(result.stdout)
        snapshot = status["snapshot"]
        assert snapshot is not None
        assert "buck2_max_rss" in snapshot

    @buck_test()
    async def test_server_snapshot_requires_status(self, buck: Buck) -> None:
        await expect_failure(buck.server("--snapshot"), stderr_regex="--status")

    @buck_test()
    @parametrize(
        "corrupt",
        ["not-json", '{"valid-json", "but-not-valid-data"}'],
    )
    async def test_corrupted_buckd_info(self, buck: Buck, corrupt: str) -> None:
        await buck.targets("//:rule")

        daemon_dir = await buck.get_daemon_dir()
        with open(f"{daemon_dir}/buckd.info") as f:
            # Check file exists and valid.
            json.load(f)

        # Kill that daemon now to avoid having making a mess and leaving 2 daemons
        # around.
        await buck.kill()

        with open(f"{daemon_dir}/buckd.info", "w") as f:
            f.write(corrupt)

        await buck.targets("//:rule")

    @buck_test()
    async def test_recovers_when_daemon_pid_cannot_be_killed(self, buck: Buck) -> None:
        # A stale buckd.info can name a pid we cannot kill (e.g. one reused by a
        # process owned by another user). Buck must report the failed kill and start
        # a fresh daemon rather than aborting, which used to leave buckd.info in
        # place so every later invocation failed the same way.
        await buck.targets("//:rule")

        daemon_dir = await buck.get_daemon_dir()
        with open(f"{daemon_dir}/buckd.info") as f:
            info = json.load(f)

        # Kill the daemon so its endpoint stops accepting connections, forcing the
        # next invocation down the "could not connect, killing daemon" path.
        await buck.kill()

        # Point buckd.info at a pid that hard_kill_until cannot kill. An out-of-range
        # pid fails the kill deterministically on every platform, standing in for the
        # reused/foreign pid from the original bug report.
        info["pid"] = 9999999999999
        with open(f"{daemon_dir}/buckd.info", "w") as f:
            json.dump(info, f)

        # Recovers by starting a new daemon; this used to fail to connect entirely.
        result = await buck.targets("//:rule")
        assert "Failed to kill buckd" in result.stderr

    @buck_test()
    async def test_process_title(self, buck: Buck) -> None:
        await buck.build()  # Start the daemon
        status = await buck.status()
        status = json.loads(status.stdout)
        pid = status["process_info"]["pid"]

        if platform.system() == "Darwin":
            out = subprocess.check_output(["ps", "-o", "comm=", str(pid)]).strip()
            assert out.startswith(b"buck2d[")
        elif platform.system() == "Linux":
            out = subprocess.check_output(["ps", "-o", "cmd=", str(pid)]).strip()
            assert out.startswith(b"buck2d[")
        elif platform.system() == "Windows":
            # We guarantee no value there.
            pass
        else:
            raise Exception("Unknown platform")

    @buck_test()
    async def test_status_fields(self, buck: Buck) -> None:
        await buck.build()  # Start the daemon
        status = await buck.status()
        status = json.loads(status.stdout)
        assert status["valid_working_directory"]
        assert status["valid_buck_out_mount"]

    @buck_test()
    async def test_status_active_commands(self, buck: Buck) -> None:
        await buck.build()  # Start the daemon

        status = json.loads((await buck.status()).stdout)
        # `status` is a oneshot request, not a registered command, so an idle
        # daemon reports no active commands.
        assert status["active_commands"] == []

        async def run_build() -> None:
            await buck.build(":long_running", "--local-only", "--no-remote-cache")

        build_task = asyncio.create_task(run_build())
        try:
            # Generous budget: under heavy CI load, daemon warm-up plus the local
            # action launch can take a while to register as an active command.
            for _ in range(600):
                status = json.loads((await buck.status()).stdout)
                if status["active_commands"]:
                    break
                if build_task.done():
                    # Surface the real failure rather than timing out below.
                    await build_task
                    raise AssertionError(
                        "Build finished without appearing in active_commands"
                    )
                await asyncio.sleep(0.1)
            else:
                raise AssertionError("Build never showed up in active_commands")

            [command] = status["active_commands"]
            assert command["trace_id"], "Active command should report its trace id"
            assert any("long_running" in arg for arg in command["argv"]), (
                f"Build argv should mention the target: {command['argv']}"
            )
            assert command["stats"] is not None
        finally:
            build_task.cancel()
            # Consume the task's outcome so an underlying error is not silently
            # dropped at teardown.
            await asyncio.gather(build_task, return_exceptions=True)

    @buck_test()
    async def test_status_all(self, buck: Buck) -> None:
        # this will start the daemons
        await buck.server()

        status = await buck.status()
        status = json.loads(status.stdout)
        pid = status["process_info"]["pid"]

        status_all = await buck.status("--all")
        status_all = json.loads(status_all.stdout)
        for status in status_all:
            if status["process_info"]["pid"] == pid:
                return
        raise Exception(
            f"buckd status for pid {pid} not found in {json.dumps(status_all, indent=2)}"
        )

    @buck_test()
    @env("BUCK_LOG", "buck2_client_ctx::daemon::client::kill=debug")
    async def test_no_buckd_kills_existing_daemon(self, buck: Buck) -> None:
        await buck.audit("cell")  # Start the daemon
        result = await buck.audit("cell", "--no-buckd")  # Kill the existing daemon
        assert "Killing daemon with PID" in result.stderr

    @buck_test()
    @env("BUCK2_TEST_INIT_DATA_SLEEP_SECS", "2")
    @env("BUCKD_STARTUP_TIMEOUT", "1")
    async def test_no_buckd_waits_for_daemon_init(self, buck: Buck) -> None:
        # Initialization takes longer than connecting to an existing daemon may.
        await buck.targets(":", "--no-buckd")

    @buck_test()
    async def test_no_buckd_is_a_noop_for_a_client_only_command(
        self, buck: Buck
    ) -> None:
        await buck.targets(":")  # Start a resident daemon
        status = json.loads((await buck.status()).stdout)
        pid = status["process_info"]["pid"]
        # `log` never connects, so it must neither kill the resident daemon nor start a
        # daemon of its own.
        await buck.log("last", "--no-buckd")
        assert daemon_is_alive(pid)

    @buck_test()
    async def test_no_buckd_without_daemon_is_quiet(self, buck: Buck) -> None:
        result = await buck.audit("cell", "--no-buckd")
        assert "no buckd server running" not in result.stderr

    @buck_test()
    async def test_no_buckd_run_starts_in_the_invocation_dir(self, buck: Buck) -> None:
        # The in-process daemon moves the client process into buck-out; the exec'd run
        # target must still start where the user invoked `buck run`.
        result = await buck.run(":pwd_run", "--no-buckd")
        assert Path(result.stdout.strip()) == Path(buck.cwd)

    @buck_test()
    async def test_no_buckd_runs_without_file_watcher(self, buck: Buck) -> None:
        # The test harness pins a file_watcher for every test; drop that line to get
        # the unconfigured default an ordinary --no-buckd invocation sees.
        extra_config = Path(buck._env["BUCK2_TEST_EXTRA_EXTERNAL_CONFIG"])
        extra_config.write_text(
            "".join(
                line
                for line in extra_config.read_text().splitlines(keepends=True)
                if "file_watcher" not in line
            )
        )
        # The in-process daemon serves exactly one command, so there is nothing to
        # invalidate between commands: it runs no file watcher and reads the tree as
        # this command's own snapshot.
        await buck.targets(":", "--no-buckd")
        spans = await filter_events(
            buck, "Event", "data", "SpanEnd", "data", "FileWatcher"
        )
        assert spans == []

    @buck_test(extra_buck_config={"buck2": {"file_watcher": "notify"}})
    async def test_no_buckd_explicit_file_watcher_wins(self, buck: Buck) -> None:
        await buck.targets(":", "--no-buckd")
        spans = await filter_events(
            buck, "Event", "data", "SpanEnd", "data", "FileWatcher"
        )
        assert spans, "an explicitly configured watcher must still run"

    @buck_test()
    async def test_no_buckd_startup_is_quiet(self, buck: Buck) -> None:
        # The in-process daemon starts up on the user's terminal, not in a buckd.stderr
        # that a tailing client would need to catch up on.
        result = await buck.targets(":", "--no-buckd")
        assert "daemon_listener" not in result.stderr

    @buck_test()
    async def test_buck_out_is_cache_dir(self, buck: Buck) -> None:
        await buck.targets(":")  # Start a daemon
        root = await buck.root()
        assert (
            (Path(root.stdout.strip()) / "buck-out" / "v2" / "CACHEDIR.TAG")
            .read_text(encoding="utf-8")
            .startswith("Signature: 8a477f597d28d172789f06886806bc55")
        )

    @buck_test()
    async def test_prev_daemon_dir(self, buck: Buck) -> None:
        buck.set_env("BUCK2_SHARE_DAEMON", "false")
        await buck.targets(":")  # Start a daemon
        await buck.kill()
        await buck.targets(":")  # Start another daemon

        def extract_pid(stderr: str) -> int:
            pid = [re.match(r".* PID: (\d+)", line) for line in stderr.splitlines()]
            pid = list(filter(None, pid))
            assert len(pid) == 1, pid[0]
            return int(pid[0].group(1))

        new_daemon_stderr = await buck.daemon_stderr()
        killed_daemon_stderr = await buck.prev_daemon_stderr()

        # check logs contain buckd pid and don't match
        assert extract_pid(new_daemon_stderr) != extract_pid(killed_daemon_stderr)

        assert "triggered shutdown: `buck kill` was invoked" in killed_daemon_stderr

    @buck_test()
    async def test_unreachable_recorded_daemon_is_replaced(self, buck: Buck) -> None:
        buck.set_env("BUCK2_SHARE_DAEMON", "false")
        await buck.targets(":")  # Start a daemon
        daemon_dir = await buck.get_daemon_dir()
        with open(daemon_dir / "buckd.info") as f:
            info = json.load(f)
        info["endpoint"] = "tcp:1"
        with open(daemon_dir / "buckd.info", "w") as f:
            json.dump(info, f)

        # `kill` cannot reach the endpoint; the next command replaces the daemon.
        await buck.kill()
        await buck.targets(":")  # Start another daemon

        def extract_pid(stderr: str) -> int:
            pid = [re.match(r".* PID: (\d+)", line) for line in stderr.splitlines()]
            pid = list(filter(None, pid))
            assert len(pid) == 1, pid[0]
            return int(pid[0].group(1))

        new_daemon_stderr = await buck.daemon_stderr()
        killed_daemon_stderr = await buck.prev_daemon_stderr()

        # check logs contain buckd pid and don't match
        assert extract_pid(new_daemon_stderr) != extract_pid(killed_daemon_stderr)

    @buck_test()
    @env("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")
    @env("BUCKD_STARTUP_INIT_TIMEOUT", "20")
    async def test_recovers_promptly_after_inactivity_shutdown(
        self, buck: Buck
    ) -> None:
        # A daemon that retires on its inactivity timeout leaves buckd.info behind
        # naming a pid that is gone. The next invocation must notice that quickly and
        # start a fresh daemon; it used to be suspected of spending the whole startup
        # budget here, which would turn an idle daemon into a 90s CLIENT_STARTUP_TIMEOUT.
        status = await buck.server("--status")
        pid = json.loads(status.stdout)["process_info"]["pid"]
        daemon_dir = await buck.get_daemon_dir()

        for _ in range(20):
            time.sleep(1)
            if not daemon_is_alive(pid):
                break
        else:
            raise AssertionError(f"Server with pid {pid} did not die in 20 seconds")

        assert "inactivity timeout elapsed" in (daemon_dir / "buckd.stderr").read_text()
        assert (daemon_dir / "buckd.info").exists(), "stale buckd.info is the point"

        start = time.time()
        result = await buck.targets("//:rule")
        elapsed = time.time() - start

        assert "buck2 daemon is not running" in result.stderr, result.stderr
        # Well inside the 20s budget. A regression into the timeout path fails the
        # command outright, so this only guards against getting slow but succeeding.
        assert elapsed < 15.0, f"took {elapsed:.2f}s to recover"

    @buck_test()
    @env("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")
    async def test_inactivity_shutdown_exits_with_a_command_in_flight(
        self, buck: Buck
    ) -> None:
        # The inactivity timer is only reset when a command *starts*, so a long lived
        # streaming command lets the timeout fire underneath itself. The daemon then
        # cannot finish draining while the client holds the stream open, and it used
        # to sit alive forever - still accepting connections it would never answer,
        # so every later invocation on this isolation dir burned its startup budget.
        status = await buck.server("--status")
        pid = json.loads(status.stdout)["process_info"]["pid"]
        daemon_dir = await buck.get_daemon_dir()

        subscriber = await buck.subscribe()
        try:
            for _ in range(20):
                time.sleep(1)
                if (
                    "inactivity timeout elapsed"
                    in (daemon_dir / "buckd.stderr").read_text()
                ):
                    break
            else:
                raise AssertionError("daemon never reached its inactivity timeout")

            for _ in range(15):
                if not daemon_is_alive(pid):
                    break
                time.sleep(1)
            else:
                raise AssertionError(
                    f"daemon {pid} announced shutdown but is still alive with a command in flight"
                )

            assert (
                "Shutdown deadline exceeded"
                in (daemon_dir / "buckd.stderr").read_text()
            )
        finally:
            # Exiting tears the stream down under the subscriber, so it reports a
            # broken connection. That is the deliberate trade: the command is
            # terminated rather than the daemon being left alive and unusable.
            with contextlib.suppress(BuckException):
                await subscriber.__aexit__(None, None, None)

    @buck_test(skip_for_os=["windows"])
    async def test_daemon_start_waits_for_tenant_outputs(self, buck: Buck) -> None:
        """
        A new daemon waits within the startup deadline for whatever still owns its tenant's
        outputs, as it waits for a running `clean`, rather than failing after a fixed time.
        """
        import fcntl  # Unix-only; this test does not run on Windows.

        buck.set_env("BUCK2_SHARE_DAEMON", "false")
        await buck.targets(":")
        daemon_dir = await buck.get_daemon_dir()
        await buck.kill()
        lock_path = daemon_dir.parent / "._buck2_tenant_locks" / daemon_dir.name
        lock_path.parent.mkdir(parents=True, exist_ok=True)
        with open(lock_path, "a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            starting = asyncio.ensure_future(buck.targets(":"))
            await asyncio.sleep(12)
            assert not starting.done(), (
                "Daemon startup gave up while the outputs were owned"
            )
        await asyncio.wait_for(starting, timeout=60)

    @buck_test(skip_for_os=["windows"])
    async def test_unshared_forkserver_state_is_reclaimed(self, buck: Buck) -> None:
        buck.set_env("BUCK2_SHARE_DAEMON", "false")
        state_root = buck.cwd / "buck-out" / "v2" / "forkserver"
        for _ in range(3):
            await buck.targets("//:rule")
            states = [entry for entry in state_root.iterdir() if entry.is_dir()]
            assert len(states) == 1
            state = states[0]
            assert state.is_dir()
            await buck.kill()
            for _ in range(100):
                if not state.exists():
                    break
                await asyncio.sleep(0.05)
            assert not state.exists()
        await buck.clean()
        assert not state_root.exists()

    @buck_test()
    async def test_unshared_daemon_preserves_shared_daemon(self, buck: Buck) -> None:
        buck.set_env("BUCK2_SHARE_DAEMON", "true")
        shared_status = json.loads((await buck.server("--status")).stdout)
        shared_pid = shared_status["process_info"]["pid"]
        isolated = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env={**buck._env, "BUCK2_SHARE_DAEMON": "false"},
            cwd=buck.cwd,
            isolation_prefix="unshared",
        )
        subscriber = await buck.subscribe()
        try:
            isolated_status = json.loads((await isolated.server("--status")).stdout)
            assert isolated_status["process_info"]["pid"] != shared_pid
            await isolated.targets("//:rule")
            await isolated.kill()
            assert daemon_is_alive(shared_pid)
            assert (
                json.loads((await buck.status()).stdout)["process_info"]["pid"]
                == shared_pid
            )
            await buck.targets("//:rule")
        finally:
            await subscriber.__aexit__(None, None, None)
            await isolated.kill()

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    @env("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "120")
    @parametrize("metadata", ["missing", "not-json", "invalid-json"])
    @parametrize("pid_file", ["unchanged", "missing", "invalid", "exited"])
    async def test_bad_shared_metadata_preserves_live_daemon(
        self, buck: Buck, metadata: str, pid_file: str
    ) -> None:
        await buck.targets("//:rule")
        sibling = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="survivor",
        )
        await sibling.targets("//:rule")
        newcomer = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="newcomer",
        )
        daemon_dir = await buck.get_daemon_dir()
        info_path = daemon_dir / "buckd.info"
        pid_path = daemon_dir / "buckd.pid"
        original_info = info_path.read_text()
        original_pid = pid_path.read_text()
        pid = json.loads(original_info)["pid"]
        damaged_pid = {
            "unchanged": original_pid,
            "invalid": "not a PID",
            "exited": str(await _exited_process_pid()),
        }.get(pid_file)
        damaged = {"missing": None, "not-json": "not JSON", "invalid-json": "[]"}[
            metadata
        ]
        try:
            if damaged is None:
                info_path.unlink()
            else:
                info_path.write_text(damaged)
            if damaged_pid is None:
                pid_path.unlink()
            else:
                pid_path.write_text(damaged_pid)
            await expect_failure(
                newcomer.targets("//:rule"),
                stderr_regex="Cannot verify shared daemon inactivity",
            )
            await expect_failure(buck.kill())
            assert daemon_is_alive(pid)
            assert (
                buck.cwd / "buck-out" / (buck.isolation_prefix or "v2") / "cache"
            ).is_dir()
            assert (buck.cwd / "buck-out" / "survivor" / "cache").is_dir()
            assert not (buck.cwd / "buck-out" / "newcomer" / "cache").exists()
            if damaged is None:
                assert not info_path.exists()
            else:
                assert info_path.read_text() == damaged
            if pid_file == "missing":
                assert not pid_path.exists()
            else:
                assert pid_path.read_text() == damaged_pid
        finally:
            info_path.write_text(original_info)
            pid_path.write_text(original_pid)
        try:
            for tenant in [buck, sibling, newcomer]:
                await tenant.targets("//:rule")
                assert (
                    json.loads((await tenant.status()).stdout)["process_info"]["pid"]
                    == pid
                )
        finally:
            await newcomer.kill()
            await sibling.kill()

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("metadata", ["missing", "not-json"])
    @parametrize("pid_file", ["unchanged", "missing", "invalid", "exited"])
    async def test_bad_shared_metadata_requires_repair_after_daemon_exit(
        self, buck: Buck, metadata: str, pid_file: str
    ) -> None:
        await buck.targets("//:rule")
        daemon_dir = await buck.get_daemon_dir()
        info_path = daemon_dir / "buckd.info"
        pid_path = daemon_dir / "buckd.pid"
        original_info = info_path.read_text()
        original_pid = pid_path.read_text()
        old_pid = json.loads(original_info)["pid"]
        await buck.kill()
        for _ in range(100):
            if not daemon_is_alive(old_pid):
                break
            await asyncio.sleep(0.1)
        assert not daemon_is_alive(old_pid)
        try:
            if metadata == "missing":
                info_path.unlink()
            else:
                info_path.write_text("not JSON")
            if pid_file == "missing":
                pid_path.unlink()
            elif pid_file == "invalid":
                pid_path.write_text("not a PID")
            elif pid_file == "exited":
                pid_path.write_text(str(await _exited_process_pid()))
            damaged_info = info_path.read_bytes() if info_path.exists() else None
            damaged_pid = pid_path.read_bytes() if pid_path.exists() else None
            await expect_failure(buck.targets("//:rule"))
            assert not daemon_is_alive(old_pid)
            assert (
                info_path.read_bytes() if info_path.exists() else None
            ) == damaged_info
            assert (pid_path.read_bytes() if pid_path.exists() else None) == damaged_pid
        finally:
            info_path.write_text(original_info)
            pid_path.write_text(original_pid)
        await buck.targets("//:rule")
        new_pid = json.loads((await buck.status()).stdout)["process_info"]["pid"]
        assert new_pid != old_pid

    @buck_test(skip_for_os=["windows"])
    @parametrize("initial_tenant", [False, True])
    @parametrize("repository_eviction", [False, True])
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("share_dice", [False, True])
    async def test_evicted_generation_cannot_overlap_outputs(
        self,
        buck: Buck,
        tmp_path: Path,
        initial_tenant: bool,
        repository_eviction: bool,
        share_dice: bool,
    ) -> None:
        startup_config = buck.cwd / ".buckconfig"
        startup_config.write_text(
            startup_config.read_text()
            + f"\n[buck2]\nsqlite_dep_file_state = true\nshare_dice = {str(share_dice).lower()}\n"
        )
        sibling = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="survivor",
        )
        tenant = (
            buck
            if initial_tenant
            else Buck(
                buck.path_to_executable,
                encoding=buck.encoding,
                env=buck._env,
                cwd=buck.cwd,
                isolation_prefix="recreated",
            )
        )
        for instance in [tenant, sibling] if initial_tenant else [sibling, tenant]:
            await instance.targets("//:rule")
        pid = json.loads((await sibling.status()).stdout)["process_info"]["pid"]
        identity = _materializer_identity(tenant)
        isolation = tenant.isolation_prefix or "v2"
        entries = json.loads((await sibling.debug("tenants", "--json")).stdout)
        generation = next(
            entry["generation"] for entry in entries if entry["isolation"] == isolation
        )
        old = _start_generation(tenant, tmp_path, "old")
        tasks = [old]
        killing = None
        try:
            await _wait_for_marker(tmp_path / "old.started", old)
            old_pid = int((tmp_path / "old.started.pid").read_text())
            assert daemon_is_alive(old_pid)
            operation = (
                tenant._run_buck_command(
                    "killall",
                    "--repo",
                    "--in-isolation-dir",
                    isolation,
                    input=None,
                    rel_cwd=None,
                    env=None,
                )
                if repository_eviction
                else tenant.kill()
            )
            killing = asyncio.ensure_future(operation)
            await _wait_for_evicted_generation(sibling, isolation, generation)
            new = _start_generation(tenant, tmp_path, "new")
            tasks.append(new)
            await asyncio.wait_for(asyncio.shield(killing), timeout=30)
            with self.assertRaises(BuckException):
                await asyncio.wait_for(asyncio.shield(old), timeout=30)
            assert not daemon_is_alive(old_pid), (
                "Eviction returned while the old action was alive"
            )
            assert not (tmp_path / "old.release").exists()
            await _wait_for_marker(tmp_path / "new.started", new)
            assert (tmp_path / "old.started").read_text() == (
                tmp_path / "new.started"
            ).read_text()
            (tmp_path / "new.release").touch()
            result = await asyncio.wait_for(asyncio.shield(new), timeout=30)
            output = result.get_build_report().output_for_target("//:generation")
            assert output.read_text() == "new"
            assert _materializer_identity(tenant) == identity
            assert (
                json.loads((await sibling.status()).stdout)["process_info"]["pid"]
                == pid
            )
        finally:
            for value in ["old", "new"]:
                (tmp_path / f"{value}.release").touch()
            await asyncio.wait_for(
                asyncio.gather(
                    *tasks, *([killing] if killing else []), return_exceptions=True
                ),
                timeout=30,
            )
            await tenant.kill()
            await sibling.kill()

    @buck_test(skip_for_os=["windows"])
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("disconnect_client", [False, True])
    @parametrize("share_dice", [False, True])
    async def test_eviction_during_generation_initialization(
        self, buck: Buck, tmp_path: Path, disconnect_client: bool, share_dice: bool
    ) -> None:
        gate = tmp_path / "initialization"
        buck.set_env("BUCK2_TEST_BLOCK_TENANT_INITIALIZATION", f"initializing,{gate}")
        startup_config = buck.cwd / ".buckconfig"
        startup_config.write_text(
            startup_config.read_text()
            + f"\n[buck2]\nshare_dice = {str(share_dice).lower()}\n"
        )
        await buck.targets("//:rule")
        pid = json.loads((await buck.status()).stdout)["process_info"]["pid"]
        tenant = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="initializing",
        )
        old = _start_generation(tenant, tmp_path, "old")
        tasks = [old]
        client = None
        try:
            await _wait_for_marker(tmp_path / "initialization.started", old)
            entries = json.loads((await buck.debug("tenants", "--json")).stdout)
            generation = next(
                entry["generation"]
                for entry in entries
                if entry["isolation"] == "initializing"
            )
            client = await tenant.kill().start()
            await _wait_for_evicted_generation(buck, "initializing", generation)
            new = _start_generation(tenant, tmp_path, "new")
            tasks.append(new)
            pending = await _wait_for_tenant_generation(
                buck, "initializing", generation
            )
            assert not pending["initialized"]
            assert not (tmp_path / "new.started").exists()
            assert client.returncode is None, (
                "Eviction returned before its I/O released ownership"
            )
            if disconnect_client:
                client.kill()
                await client.communicate()
            await buck.targets("//:rule")
            assert not (tmp_path / "new.started").exists()
            (tmp_path / "initialization.release").touch()
            if not disconnect_client:
                _, stderr = await client.communicate()
                assert client.returncode == 0, stderr.decode()
            with self.assertRaisesRegex(BuckException, "evicted"):
                await asyncio.wait_for(asyncio.shield(old), timeout=30)
            await _wait_for_marker(tmp_path / "new.started", new)
            (tmp_path / "new.release").touch()
            result = await asyncio.wait_for(asyncio.shield(new), timeout=30)
            assert (
                result.get_build_report().output_for_target("//:generation").read_text()
                == "new"
            )
            assert (
                json.loads((await buck.status()).stdout)["process_info"]["pid"] == pid
            )
        finally:
            (tmp_path / "initialization.release").touch()
            for value in ["old", "new"]:
                (tmp_path / f"{value}.release").touch()
            if client is not None:
                if client.returncode is None:
                    with contextlib.suppress(ProcessLookupError):
                        client.kill()
                await client.communicate()
            await asyncio.wait_for(
                asyncio.gather(*tasks, return_exceptions=True), timeout=30
            )
            await tenant.kill()
            await buck.kill()

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("share_dice", [False, True])
    async def test_isolated_tenants_share_daemon_without_sharing_config(
        self, buck: Buck, tmp_path: Path, share_dice: bool
    ) -> None:
        startup_config = buck.cwd / ".buckconfig"
        startup_config.write_text(
            startup_config.read_text()
            + f"\n[buck2]\nshare_dice = {str(share_dice).lower()}\n"
        )
        config_one = tmp_path / "tenant-one.bcfg"
        config_one.write_text("[tenant]\n  value = one\n")
        config_two = tmp_path / "tenant-two.bcfg"
        config_two.write_text("[tenant]\n  value = two\n")

        tenant_one = _isolated_buck(buck, "tenant-one")
        tenant_two = _isolated_buck(buck, "tenant-two")

        async def config_value(tenant: Buck, config: Path) -> str:
            result = await tenant.audit_config(
                "tenant.value", "--style=json", "--config-file", str(config)
            )
            return result.get_json()["tenant.value"]

        assert await config_value(tenant_one, config_one) == "one"
        assert await config_value(tenant_two, config_two) == "two"

        tenant_one_pid = json.loads((await tenant_one.status()).stdout)["process_info"][
            "pid"
        ]
        tenant_two_pid = json.loads((await tenant_two.status()).stdout)["process_info"][
            "pid"
        ]
        assert tenant_one_pid == tenant_two_pid

        tenants = json.loads((await tenant_one.debug("tenants", "--json")).stdout)
        tenant_one_info = next(
            tenant for tenant in tenants if tenant["isolation"] == "tenant-one"
        )
        assert any(tenant["isolation"] == "tenant-two" for tenant in tenants)

        await tenant_one.kill()
        assert await config_value(tenant_two, config_two) == "two"
        assert (
            json.loads((await tenant_two.status()).stdout)["process_info"]["pid"]
            == tenant_two_pid
        )

        assert await config_value(tenant_one, config_one) == "one"
        recreated_tenants = json.loads(
            (await tenant_one.debug("tenants", "--json")).stdout
        )
        recreated_tenant_one = next(
            tenant
            for tenant in recreated_tenants
            if tenant["isolation"] == "tenant-one"
        )
        assert recreated_tenant_one["generation"] > tenant_one_info["generation"]

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_unreachable_shared_daemon_is_not_killed(self, buck: Buck) -> None:
        await buck.targets("//:rule")
        sibling = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="sibling",
        )
        await sibling.targets("//:rule")
        daemon_dir = await buck.get_daemon_dir()
        info_path = daemon_dir / "buckd.info"
        original = info_path.read_text()
        info = json.loads(original)
        pid = info["pid"]
        info["endpoint"] = "tcp:1"
        info_path.write_text(json.dumps(info))
        try:
            await expect_failure(buck.kill())
            await expect_failure(
                buck.targets("//:rule"), stderr_regex="Shared daemon is alive"
            )
            cache = buck.cwd / "buck-out" / (buck.isolation_prefix or "v2") / "cache"
            assert cache.exists()
            await expect_failure(buck.clean())
            assert cache.exists()
            assert daemon_is_alive(pid)
        finally:
            info_path.write_text(original)
        await sibling.targets("//:rule")
        await sibling.kill()

    @buck_test()
    @parametrize("sharing", ["false", "true"])
    async def test_killall_repo_reaches_all_isolation_directories(
        self, buck: Buck, sharing: str
    ) -> None:
        buck.set_env("BUCK2_SHARE_DAEMON", sharing)
        tenants = [
            Buck(
                buck.path_to_executable,
                encoding=buck.encoding,
                env=buck._env,
                cwd=buck.cwd,
                isolation_prefix=isolation,
            )
            for isolation in ["tenant-one", "tenant-two"]
        ]
        try:
            for tenant in tenants:
                await tenant.targets("//:rule")
            await buck._run_buck_command(
                "killall",
                "--repo",
                "--in-isolation-dir",
                "tenant-one",
                input=None,
                rel_cwd=None,
                env={"BUCK2_SHARE_DAEMON": sharing},
            )
            if sharing == "false":
                assert "no buckd running" in (await tenants[0].status()).stderr
            else:
                registered = json.loads(
                    (await tenants[1].debug("tenants", "--json")).stdout
                )
                assert not any(t["isolation"] == "tenant-one" for t in registered)
            await tenants[1].targets("//:rule")
            await buck._run_buck_command(
                "killall",
                "--repo",
                input=None,
                rel_cwd=None,
                env={"BUCK2_SHARE_DAEMON": sharing},
            )
            for tenant in tenants:
                assert "no buckd running" in (await tenant.status()).stderr
            await buck._run_buck_command(
                "killall",
                "--repo",
                input=None,
                rel_cwd=None,
                env={"BUCK2_SHARE_DAEMON": sharing},
            )
        finally:
            for tenant in tenants:
                await tenant.kill()

    @buck_test(skip_for_os=["windows"])
    @parametrize("owner_sharing", ["false", "true"])
    @parametrize("isolation", ["collision", "v2"])
    async def test_mixed_daemon_modes_cannot_own_same_outputs(
        self, buck: Buck, tmp_path: Path, owner_sharing: str, isolation: str
    ) -> None:
        buck.set_env("BUCK2_SHARE_DAEMON", owner_sharing)
        buck.set_isolation_prefix(isolation)
        other = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env={
                **buck._env,
                "BUCK2_SHARE_DAEMON": "true" if owner_sharing == "false" else "false",
            },
            cwd=buck.cwd,
            isolation_prefix=isolation,
        )
        old = _start_generation(buck, tmp_path, "old")
        try:
            await _wait_for_marker(tmp_path / "old.started", old)
            new = _start_generation(other, tmp_path, "new")
            # A shared intruder's daemon cannot take the tenant's disk lock; an unshared intruder
            # cannot take the lifecycle lock that the shared owner holds for the tenant.
            await expect_failure(
                new,
                stderr_regex=(
                    "another process still owns it"
                    if owner_sharing == "false"
                    else "Error locking buckd.lifecycle"
                ),
            )
            assert not (tmp_path / "new.started").exists()
            (tmp_path / "old.release").touch()
            result = await asyncio.wait_for(asyncio.shield(old), timeout=30)
            output = result.get_build_report().output_for_target("//:generation")
            assert output.read_text() == "old"
            await buck.kill()
            (tmp_path / "new.release").touch()
            result = await asyncio.wait_for(
                _start_generation(other, tmp_path, "new"), timeout=30
            )
            new_output = result.get_build_report().output_for_target("//:generation")
            assert new_output == output
            assert new_output.read_text() == "new"
            repeated = await other.build(
                "//:generation",
                "--config-file",
                str(tmp_path / "new.bcfg"),
                "--no-remote-cache",
            )
            assert (
                repeated.get_build_report()
                .output_for_target("//:generation")
                .read_text()
                == "new"
            )
        finally:
            for value in ["old", "new"]:
                (tmp_path / f"{value}.release").touch()
            await asyncio.wait_for(
                asyncio.gather(old, return_exceptions=True), timeout=30
            )
            await other.kill()

    @buck_test()
    @parametrize("sharing", ["false", "true"])
    @parametrize("background", [False, True])
    @parametrize("dry_run", [False, True])
    async def test_clean_preserves_sibling_tenant(
        self, buck: Buck, tmp_path: Path, sharing: str, background: bool, dry_run: bool
    ) -> None:
        buck.set_env("BUCK2_SHARE_DAEMON", sharing)
        sibling = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="survivor",
        )
        (tmp_path / "value.release").touch()
        result = await _start_generation(buck, tmp_path, "value")
        output = result.get_build_report().output_for_target("//:generation")
        try:
            result = await _start_generation(sibling, tmp_path, "value")
            sibling_output = result.get_build_report().output_for_target(
                "//:generation"
            )
            pid = json.loads((await sibling.status()).stdout)["process_info"]["pid"]
            args = (["--background"] if background else []) + (
                ["--dry-run"] if dry_run else []
            )
            await buck.clean(*args)
            assert output.exists() == dry_run
            assert sibling_output.read_text() == "value"
            assert (
                json.loads((await sibling.status()).stdout)["process_info"]["pid"]
                == pid
            )
            await sibling.build(
                "//:generation",
                "--config-file",
                str(tmp_path / "value.bcfg"),
                "--no-remote-cache",
            )
            assert sibling_output.read_text() == "value"
            (tmp_path / "after.release").touch()
            after = await _start_generation(sibling, tmp_path, "after")
            assert (
                after.get_build_report().output_for_target("//:generation").read_text()
                == "after"
            )
            result = await _start_generation(buck, tmp_path, "value")
            assert (
                result.get_build_report().output_for_target("//:generation").read_text()
                == "value"
            )
        finally:
            await sibling.kill()

    @buck_test(skip_for_os=["windows"])
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("background", [False, True])
    async def test_clean_waits_for_tenant_disk_work(
        self, buck: Buck, tmp_path: Path, background: bool
    ) -> None:
        gate = tmp_path / "initialization"
        isolation = buck.isolation_prefix or "v2"
        buck.set_env("BUCK2_TEST_BLOCK_TENANT_INITIALIZATION", f"{isolation},{gate}")
        sibling = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="survivor",
        )
        await sibling.targets("//:rule")
        pid = json.loads((await sibling.status()).stdout)["process_info"]["pid"]
        old = _start_generation(buck, tmp_path, "old")
        cleaning = None
        try:
            await _wait_for_marker(tmp_path / "initialization.started", old)
            entries = json.loads((await sibling.debug("tenants", "--json")).stdout)
            generation = next(
                entry["generation"]
                for entry in entries
                if entry["isolation"] == isolation
            )
            sentinel = buck.cwd / "buck-out" / isolation / "preserved"
            sentinel.write_text("old")
            cleaning = asyncio.ensure_future(
                buck.clean(*(["--background"] if background else []))
            )
            await _wait_for_evicted_generation(sibling, isolation, generation)
            assert not cleaning.done()
            assert sentinel.exists(), (
                "Clean deleted outputs still owned by cancelled I/O"
            )
            (tmp_path / "initialization.release").touch()
            with self.assertRaisesRegex(BuckException, "evicted"):
                await asyncio.wait_for(asyncio.shield(old), timeout=30)
            await asyncio.wait_for(asyncio.shield(cleaning), timeout=30)
            assert not sentinel.exists()
            assert (
                json.loads((await sibling.status()).stdout)["process_info"]["pid"]
                == pid
            )
        finally:
            (tmp_path / "initialization.release").touch()
            (tmp_path / "old.release").touch()
            await asyncio.wait_for(
                asyncio.gather(
                    old, *([cleaning] if cleaning else []), return_exceptions=True
                ),
                timeout=35,
            )
            await sibling.kill()

    @buck_test()
    @env("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "120")
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("sharing", ["false", "true"])
    @parametrize("fault", ["unreachable", "stale-pid", "rpc", "missing-info"])
    async def test_repository_eviction_reports_and_continues_after_failures(
        self, buck: Buck, fault: str, sharing: str
    ) -> None:
        tenants = [
            Buck(
                buck.path_to_executable,
                encoding=buck.encoding,
                env={**buck._env, "BUCK2_SHARE_DAEMON": sharing if i == 0 else "false"},
                cwd=buck.cwd,
                isolation_prefix=name,
            )
            for i, name in enumerate(["tenant-one", "tenant-two"])
        ]
        for tenant in tenants:
            await tenant.targets("//:rule")
        info_path = (await tenants[0].get_daemon_dir()) / "buckd.info"
        original = info_path.read_text()
        info = json.loads(original)
        pid = info["pid"]
        damaged_paths = [info_path]
        saved = {path: path.read_bytes() for path in damaged_paths}
        empty = info_path.parent.parent / "empty"
        empty.mkdir()
        if fault in ["unreachable", "stale-pid"]:
            info["endpoint"] = "tcp:1"
            if fault == "stale-pid":
                info["pid"] = await _exited_process_pid()
            info_path.write_text(json.dumps(info))
        elif fault == "missing-info":
            for path in damaged_paths:
                path.unlink()
        args = ["--repo"] + (
            ["--in-isolation-dir", "invalid/name"] if fault == "rpc" else []
        )
        try:
            result = await expect_failure(
                buck._run_buck_command(
                    "killall", *args, input=None, rel_cwd=None, env=None
                ),
                stderr_regex="Repository tenant eviction failed",
            )
            if sharing == "true" and fault in ["unreachable", "stale-pid"]:
                await expect_failure(tenants[0].kill())
            assert daemon_is_alive(pid)
            if fault != "rpc":
                if fault in ["unreachable", "stale-pid"]:
                    assert "Failed to contact daemon" in result.stderr
                else:
                    assert "buckd.info is missing" in result.stderr
                    assert all(not path.exists() for path in damaged_paths)
                assert "failed for 1 daemon(s)" in result.stderr
                assert "Evicted 1 tenant(s)" in result.stderr
                assert "no buckd running" in (await tenants[1].status()).stderr
            else:
                assert "failed for 2 daemon(s)" in result.stderr
                assert result.stderr.count("Failed to evict tenants from daemon") == 2
                assert (
                    str(info_path.parent) in result.stderr
                    and "tenant-two" in result.stderr
                )
        finally:
            for path, contents in saved.items():
                path.write_bytes(contents)
            for tenant in tenants:
                await tenant.kill()

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_shared_startup_mismatch_preserves_siblings(self, buck: Buck) -> None:
        await buck.targets("//:rule")
        sibling = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env=buck._env,
            cwd=buck.cwd,
            isolation_prefix="sibling",
        )
        try:
            await sibling.targets("//:rule")
            pid = json.loads((await sibling.status()).stdout)["process_info"]["pid"]
            config = buck.cwd / ".buckconfig"
            original = config.read_text()
            try:
                config.write_text(original + "\n[buck2]\ndaemon_buster = mismatch\n")
                await expect_failure(
                    buck.targets("//:rule"),
                    stderr_regex="Shared daemon constraint mismatch",
                )
                assert daemon_is_alive(pid)
            finally:
                config.write_text(original)
            await sibling.targets("//:rule")
            assert (
                json.loads((await sibling.status()).stdout)["process_info"]["pid"]
                == pid
            )
        finally:
            await sibling.kill()

    @buck_test()
    async def test_host_killall_isolation_filter_preserves_other_daemons(
        self,
        buck: Buck,
    ) -> None:
        prefix = f"tenant-filter-{uuid4().hex}"
        tenants = [
            Buck(
                buck.path_to_executable,
                encoding=buck.encoding,
                env={**buck._env, "BUCK2_SHARE_DAEMON": "false"},
                cwd=buck.cwd,
                isolation_prefix=f"{prefix}-{name}",
            )
            for name in ["selected", "survivor"]
        ]
        try:
            for tenant in tenants:
                await tenant.targets("//:rule")
            pid = json.loads((await tenants[1].status()).stdout)["process_info"]["pid"]
            await tenants[0]._run_buck_command(
                "killall",
                "--in-isolation-dir",
                f"{prefix}-selected",
                input=None,
                rel_cwd=None,
                env=None,
            )
            assert "no buckd running" in (await tenants[0].status()).stderr
            await tenants[1].targets("//:rule")
            assert (
                json.loads((await tenants[1].status()).stdout)["process_info"]["pid"]
                == pid
            )
        finally:
            for tenant in tenants:
                await tenant.kill()

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_live_legacy_owner_blocks_shared_initialization(
        self, buck: Buck
    ) -> None:
        buck.set_isolation_prefix("legacy")
        shared_dir = await buck.get_daemon_dir()
        legacy_dir = shared_dir.parent / "legacy"
        legacy_dir.mkdir(parents=True, exist_ok=True)
        pid_file = legacy_dir / "buckd.pid"
        pid_file.write_text(str(os.getpid()))
        try:
            await expect_failure(
                buck.targets("//:rule"), stderr_regex="already owned by a legacy daemon"
            )
            assert not (buck.cwd / "buck-out" / "legacy" / "cache").exists()
        finally:
            pid_file.unlink()

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_clean_cannot_unlink_sibling_disk_lock(self, buck: Buck) -> None:
        sibling = Buck(
            buck.path_to_executable,
            encoding=buck.encoding,
            env={**buck._env},
            cwd=buck.cwd,
            isolation_prefix="sibling",
        )
        try:
            await sibling.targets("//:rule")
            daemon_dir = await sibling.get_daemon_dir()
            lock_files = [
                path / "sibling"
                for path in daemon_dir.parent.iterdir()
                if path.is_dir() and (path / "sibling").is_file()
            ]
            assert len(lock_files) == 1
            inode = lock_files[0].stat().st_ino
            buck.set_env("BUCK2_SHARE_DAEMON", "false")
            buck.set_isolation_prefix(".tenant-locks")
            await buck.clean()
            assert lock_files[0].stat().st_ino == inode
            await sibling.targets("//:rule")
        finally:
            await sibling.kill()

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize(
        "bad_pid",
        ["", "not a PID", "0", "-1", "99999999999999999999"],
        ids=["empty", "garbage", "zero", "negative", "overflow"],
    )
    async def test_garbage_legacy_pid_refuses_shared_initialization(
        self, buck: Buck, bad_pid: str
    ) -> None:
        buck.set_isolation_prefix("legacy-garbage")
        shared_dir = await buck.get_daemon_dir()
        legacy_dir = shared_dir.parent / "legacy-garbage"
        legacy_dir.mkdir(parents=True, exist_ok=True)
        pid_file = legacy_dir / "buckd.pid"
        pid_file.write_text(bad_pid)
        sentinel = buck.cwd / "buck-out" / "legacy-garbage" / "sentinel.txt"
        sentinel.parent.mkdir(parents=True, exist_ok=True)
        sentinel.write_text("sentinel")
        try:
            failure = await expect_failure(
                buck.targets("//:rule"),
                stderr_regex="Cannot verify legacy daemon ownership",
            )
            assert "restore a consistent" in failure.stderr
            assert "Stop the legacy daemon" in failure.stderr
            assert pid_file.read_text() == bad_pid
            assert sentinel.read_text() == "sentinel"
            assert not (buck.cwd / "buck-out" / "legacy-garbage" / "cache").exists()
            pid_file.write_text(str(await _exited_process_pid()))
            await buck.targets("//:rule")
            assert (buck.cwd / "buck-out" / "legacy-garbage" / "cache").is_dir()
        finally:
            pid_file.unlink(missing_ok=True)

    @buck_test()
    @env("BUCK2_SHARE_DAEMON", "true")
    @parametrize("legacy_pid", ["missing", "exited"])
    async def test_missing_and_exited_legacy_pid_allows_shared_initialization(
        self, buck: Buck, legacy_pid: str
    ) -> None:
        buck.set_isolation_prefix("legacy-allowed")
        shared_dir = await buck.get_daemon_dir()
        legacy_dir = shared_dir.parent / "legacy-allowed"
        legacy_dir.mkdir(parents=True, exist_ok=True)
        pid_file = legacy_dir / "buckd.pid"
        try:
            if legacy_pid == "exited":
                pid_file.write_text(str(await _exited_process_pid()))
            else:
                pid_file.unlink(missing_ok=True)
            await buck.targets("//:rule")
            assert (buck.cwd / "buck-out" / "legacy-allowed" / "cache").is_dir()
        finally:
            pid_file.unlink(missing_ok=True)

    @buck_test()
    @parametrize("share_dice", [False, True])
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_isolated_tenants_keep_cached_paths_local(
        self, buck: Buck, tmp_path: Path, share_dice: bool
    ) -> None:
        startup_config = buck.cwd / ".buckconfig"
        startup_config.write_text(
            startup_config.read_text()
            + f"\n[buck2]\nshare_dice = {str(share_dice).lower()}\n"
        )
        tenants = [_isolated_buck(buck, name) for name in ["tenant-one", "tenant-two"]]
        try:
            outputs = []
            for tenant in tenants:
                result = await tenant.build("//:tenant_value")
                output = result.get_build_report().output_for_target("//:tenant_value")
                assert output.read_text() == "unset"
                outputs.append(output)
            assert outputs[0] != outputs[1]
            pids = [
                json.loads((await tenant.status()).stdout)["process_info"]["pid"]
                for tenant in tenants
            ]
            assert pids[0] == pids[1]
            await _assert_tenant_cached_paths(tenants)
            await _assert_tenant_dice_graphs(
                tenants[0], tenants[1], tmp_path, share_dice
            )
        finally:
            for tenant in tenants:
                await tenant.kill()
        await _assert_daemon_exits(pids[0])

    @buck_test()
    @parametrize("share_dice", [False, True])
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_isolated_tenants_select_dice_engine_ownership(
        self, buck: Buck, tmp_path: Path, share_dice: bool
    ) -> None:
        startup_config = buck.cwd / ".buckconfig"
        startup_config.write_text(
            startup_config.read_text()
            + f"\n[buck2]\nshare_dice = {str(share_dice).lower()}\n"
        )
        config_one = tmp_path / "tenant-one.bcfg"
        config_one.write_text("[tenant]\nvalue = one\n")
        config_two = tmp_path / "tenant-two.bcfg"
        config_two.write_text("[tenant]\nvalue = two\n")
        tenant_one = _isolated_buck(buck, "tenant-one")
        tenant_two = _isolated_buck(buck, "tenant-two")
        try:
            output_one = await _tenant_value(tenant_one, config_one)
            output_two = await _tenant_value(tenant_two, config_two)
            assert output_one != output_two
            assert output_one.read_text() == "one"
            assert output_two.read_text() == "two"
            daemon_pid = json.loads((await tenant_one.status()).stdout)["process_info"][
                "pid"
            ]
            assert (
                daemon_pid
                == json.loads((await tenant_two.status()).stdout)["process_info"]["pid"]
            )
            await _assert_tenant_dice_graphs(
                tenant_one, tenant_two, tmp_path, share_dice
            )
            assert (await _tenant_value(tenant_one, config_one)).read_text() == "one"
            assert (await _tenant_value(tenant_two, config_two)).read_text() == "two"
        finally:
            await tenant_one.kill()
            await tenant_two.kill()
        await _assert_daemon_exits(daemon_pid)

    @buck_test(skip_for_os=["darwin", "windows"])
    @parametrize("share_dice", [False, True])
    @parametrize("same_commit", [False, True])
    @env("BUCK2_SHARE_DAEMON", "true")
    async def test_checkouts_share_daemon(
        self, buck: Buck, tmp_path: Path, share_dice: bool, same_commit: bool
    ) -> None:
        source = tmp_path / "source"
        source.mkdir()
        for name in [".buckconfig", "TARGETS.fixture", "defs.bzl", "paths.bxl"]:
            shutil.copyfile(buck.cwd / name, source / name)
        (source / ".buckroot").touch()
        startup_config = source / ".buckconfig"
        startup_config.write_text(
            startup_config.read_text()
            + f"\n[buck2]\nshare_dice = {str(share_dice).lower()}\n"
            + "file_watcher = fs_hash_crawler\n"
        )
        targets = source / "TARGETS.fixture"
        template = targets.read_text()

        def hg(cwd: Path, *args: str) -> str:
            return subprocess.run(
                ["hg", *args], cwd=cwd, check=True, stdout=subprocess.PIPE, text=True
            ).stdout.strip()

        hg(source, "init")
        revisions = []
        for value in ["commit-one", "commit-two"]:
            targets.write_text(
                template.replace(
                    'read_config("tenant", "value", "unset")', f'"{value}"'
                )
            )
            if not revisions:
                hg(
                    source,
                    "add",
                    ".buckconfig",
                    ".buckroot",
                    "TARGETS.fixture",
                    "defs.bzl",
                    "paths.bxl",
                )
            hg(source, "commit", "-m", value, "-u", "Buck2 E2E")
            revisions.append(hg(source, "log", "-r", ".", "-T", "{node}"))
        assert revisions[0] != revisions[1]
        revisions[1] = revisions[0] if same_commit else revisions[1]
        values = ["commit-one", "commit-one" if same_commit else "commit-two"]

        tenants = []
        for name, revision in zip(["checkout-one", "checkout-two"], revisions):
            checkout = tmp_path / name
            hg(source, "clone", "--updaterev", revision, str(source), str(checkout))
            assert hg(checkout, "log", "-r", ".", "-T", "{node}") == revision
            assert not hg(
                checkout, "status", "--modified", "--added", "--removed", "--deleted"
            )
            tenants.append(
                Buck(
                    buck.path_to_executable,
                    encoding=buck.encoding,
                    env={
                        **buck._env,
                        "BUCK2_TEST_HOME_DIR": str(tmp_path / "home"),
                        "BUCK2_DAEMON_GROUP": "different-commits",
                    },
                    cwd=checkout,
                    isolation_prefix="checkout",
                )
            )
        tenant_one, tenant_two = tenants

        async def build_value(tenant: Buck) -> Path:
            result = await tenant.build("//:tenant_value")
            return result.get_build_report().output_for_target("//:tenant_value")

        try:
            outputs = []
            for tenant, value in zip(tenants, values):
                output = await build_value(tenant)
                assert output.read_text() == value
                assert output.is_relative_to(tenant.cwd)
                outputs.append(output)
            assert outputs[0] != outputs[1]
            daemon_pid = json.loads((await tenant_one.status()).stdout)["process_info"][
                "pid"
            ]
            assert (
                json.loads((await tenant_two.status()).stdout)["process_info"]["pid"]
                == daemon_pid
            )
            await _assert_tenant_dice_graphs(
                tenant_one, tenant_two, tmp_path, share_dice
            )

            await _assert_tenant_cached_paths(tenants)

            startup_config = tenant_two.cwd / ".buckconfig"
            original_config = startup_config.read_text()
            startup_config.write_text(
                original_config
                + f"\n[buck2]\nshare_dice = {str(not share_dice).lower()}\n"
            )
            other_group = Buck(
                buck.path_to_executable,
                encoding=buck.encoding,
                env={**tenant_two._env, "BUCK2_DAEMON_GROUP": "other-mode"},
                cwd=tenant_two.cwd,
                isolation_prefix="other-mode",
            )
            try:
                await expect_failure(
                    tenant_two.build("//:tenant_value"),
                    stderr_regex="daemon group .* has incompatible constraints",
                )
                assert (await build_value(other_group)).read_text() == values[1]
                assert (
                    json.loads((await other_group.status()).stdout)["process_info"][
                        "pid"
                    ]
                    != daemon_pid
                )
            finally:
                await other_group.kill()
                startup_config.write_text(original_config)
            assert (
                json.loads((await tenant_one.status()).stdout)["process_info"]["pid"]
                == daemon_pid
            )

            for tenant, value in zip(tenants, values):
                assert (await build_value(tenant)).read_text() == value
            await tenant_one.kill()
            assert (await build_value(tenant_two)).read_text() == values[1]
            assert (
                json.loads((await tenant_two.status()).stdout)["process_info"]["pid"]
                == daemon_pid
            )
            assert (await build_value(tenant_one)).read_text() == "commit-one"
            assert (
                json.loads((await tenant_one.status()).stdout)["process_info"]["pid"]
                == daemon_pid
            )
            for tenant, revision in zip(tenants, revisions):
                assert hg(tenant.cwd, "log", "-r", ".", "-T", "{node}") == revision
                assert not hg(
                    tenant.cwd,
                    "status",
                    "--modified",
                    "--added",
                    "--removed",
                    "--deleted",
                )
        finally:
            for tenant in tenants:
                await tenant.kill()
        await _assert_daemon_exits(daemon_pid)


async def _wait_for_marker(path: Path, task: asyncio.Task[BuildResult]) -> None:
    async def wait() -> None:
        while not path.exists():
            if task.done():
                task.result()
                raise AssertionError(f"Build exited without creating {path}")
            await asyncio.sleep(0.05)

    await asyncio.wait_for(wait(), timeout=30)


def _start_generation(
    buck: Buck, directory: Path, value: str
) -> asyncio.Task[BuildResult]:
    config = directory / f"{value}.bcfg"
    config.write_text(
        f"[generation]\nstarted = {directory / f'{value}.started'}\n"
        f"release = {directory / f'{value}.release'}\nvalue = {value}\n"
    )

    async def build() -> BuildResult:
        return await buck.build(
            "//:generation", "--config-file", str(config), "--no-remote-cache"
        )

    return asyncio.create_task(build())


async def _wait_for_tenant_generation(
    buck: Buck, isolation: str, generation: int
) -> dict:
    async def wait() -> dict:
        while True:
            tenants = json.loads((await buck.debug("tenants", "--json")).stdout)
            entry = next(
                (entry for entry in tenants if entry["isolation"] == isolation), None
            )
            if entry is not None and entry["generation"] > generation:
                return entry
            await asyncio.sleep(0.05)

    return await asyncio.wait_for(wait(), timeout=30)


async def _wait_for_evicted_generation(
    buck: Buck, isolation: str, generation: int
) -> None:
    async def wait() -> None:
        while any(
            entry["isolation"] == isolation and entry["generation"] == generation
            for entry in json.loads((await buck.debug("tenants", "--json")).stdout)
        ):
            await asyncio.sleep(0.05)

    await asyncio.wait_for(wait(), timeout=30)


def _materializer_identity(buck: Buck) -> str:
    db = (
        buck.cwd
        / "buck-out"
        / (buck.isolation_prefix or "v2")
        / "cache/materializer_state/db.sqlite"
    )
    with sqlite3.connect(db) as connection:
        return connection.execute(
            "SELECT value FROM created_by WHERE key = 'timestamp_on_initialization'"
        ).fetchone()[0]


async def _exited_process_pid() -> int:
    process = await asyncio.create_subprocess_exec(sys.executable, "-c", "pass")
    assert await process.wait() == 0
    assert not daemon_is_alive(process.pid)
    return process.pid


def _isolated_buck(buck: Buck, isolation: str) -> Buck:
    return Buck(
        buck.path_to_executable,
        encoding=buck.encoding,
        env=buck._env,
        cwd=buck.cwd,
        isolation_prefix=isolation,
    )


async def _dice_analysis_keys(buck: Buck, path: Path) -> dict[str, int]:
    await buck.debug("dice-dump", "--path", str(path))
    with gzip.open(path / "nodes.gz", "rt") as nodes:
        rows = [line.rstrip("\n").split("\t") for line in nodes]
    return {
        key.partition(" (")[0]: int(index)
        for index, kind, key in rows
        if kind == "AnalysisKey"
    }


async def _tenant_value(buck: Buck, config: Path) -> Path:
    result = await buck.build(
        "//:tenant_value", "--config-file", str(config), "--build-report=-"
    )
    return result.get_build_report().output_for_target("//:tenant_value")


async def _assert_tenant_cached_paths(tenants: list[Buck]) -> None:
    outputs = []
    for first_run in [True, False]:
        for tenant in tenants:
            result = await tenant.bxl("//paths.bxl:paths")
            root_json, output_path = result.stdout.splitlines()
            paths = json.loads(root_json)
            assert paths["root"] == str(tenant.cwd)
            output = Path(output_path)
            isolation = tenant.isolation_prefix
            assert isolation is not None
            assert output.is_relative_to(tenant.cwd / "buck-out" / isolation)
            assert output.read_text() == str(tenant.cwd)
            assert ("evaluating tenant paths" in result.stderr) == first_run
            if first_run:
                outputs.append(output)
    assert len(set(outputs)) == len(tenants)


async def _assert_daemon_exits(pid: int) -> None:
    for _ in range(200):
        if not daemon_is_alive(pid):
            return
        await asyncio.sleep(0.05)
    assert not daemon_is_alive(pid), "Daemon survived final tenant eviction"


async def _assert_tenant_dice_graphs(
    tenant_one: Buck, tenant_two: Buck, tmp_path: Path, share_dice: bool
) -> None:
    probe_one = "root//:tenant_one_probe"
    probe_two = "root//:tenant_two_probe"
    await tenant_one.build("//:tenant_one_probe")
    baseline = await _dice_analysis_keys(tenant_one, tmp_path / "before")
    assert probe_one in baseline
    assert probe_two not in baseline

    await tenant_two.build("//:tenant_two_probe")
    event_log = await tenant_two.log("show", "--recent", "0")
    assert f"dice-sharing:{str(share_dice).lower()}" in event_log.stdout
    keys_one = await _dice_analysis_keys(tenant_one, tmp_path / "after-one")
    keys_two = await _dice_analysis_keys(tenant_two, tmp_path / "after-two")
    assert probe_one in keys_one
    assert probe_two in keys_two
    if share_dice:
        assert probe_one in keys_one
        assert probe_two in keys_one
        assert probe_one in keys_two
        assert probe_two in keys_two
    else:
        assert probe_two not in keys_one
        assert probe_one not in keys_two
