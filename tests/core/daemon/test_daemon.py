# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import asyncio
import contextlib
import json
import platform
import re
import subprocess
import time
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckException
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import (
    buck_test,
    BuckTestCase,
    env,
    parametrize,
)
from buck2.tests.e2e_util.helper.utils import daemon_is_alive


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
