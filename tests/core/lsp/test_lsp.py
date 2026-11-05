# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import asyncio
import json
import os
import signal
from pathlib import Path
from typing import Any, Optional

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.fixtures import Fixture, Span
from buck2.tests.e2e_util.api.lsp import LSPResponseError
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env
from buck2.tests.e2e_util.helper.utils import daemon_is_alive


def _assert_range(range: dict[str, Any], expected: Optional[Span]) -> None:
    """Assert that this Span is equal to an LSP range dict"""
    if expected is None:
        expected = Span(0, 0, 0, 0)
    assert range["start"]["line"] == expected.start_line
    assert range["start"]["character"] == expected.start_col
    assert range["end"]["line"] == expected.end_line
    assert range["end"]["character"] == expected.end_col


def _assert_uris(actual: str, expected: str) -> None:
    if os.name == "nt":
        # Windows file paths are case-insensitive, and the LSP returns the drive identifier in upper-case.
        # Windows also allows paths to use forward and backward slashes interchangeably.
        # Normalize the paths only on Windows to avoid flakiness.
        assert actual.replace("\\", "/").replace("%3A", ":").lower() == expected.lower()
    else:
        assert actual == expected


def _assert_goto_result(
    res: list[dict[str, Any]],
    expected_src: Span,
    expected_dest_path: Path,
    expected_dest: Optional[Span],
) -> None:
    assert len(res) == 1
    _assert_range(res[0]["originSelectionRange"], expected_src)
    _assert_range(res[0]["targetRange"], expected_dest)
    _assert_range(res[0]["targetSelectionRange"], expected_dest)
    _assert_uris(res[0]["targetUri"], expected_dest_path.as_uri())


def fixture(buck: Buck, path: Path) -> Fixture:
    abs_path = buck.cwd / path
    fixture = Fixture(abs_path.read_text())
    abs_path.write_text(fixture.content)
    return fixture


async def _wait_for_exit(process: asyncio.subprocess.Process, timeout: float) -> bool:
    try:
        await asyncio.wait_for(process.wait(), timeout=timeout)
        return True
    except TimeoutError:
        return False


async def _kill_if_alive(process: asyncio.subprocess.Process) -> None:
    if process.returncode is not None:
        return

    process.kill()
    await asyncio.wait_for(process.wait(), timeout=30)


async def _wait_for_file_to_contain(
    path: Path,
    substring: str,
    timeout: float,
) -> bool:
    deadline = asyncio.get_running_loop().time() + timeout
    while asyncio.get_running_loop().time() < deadline:
        if path.exists() and substring in path.read_text():
            return True
        await asyncio.sleep(1)
    return False


def _active_commands_snapshot_has_command(
    msg: dict[str, Any],
    command_name: str,
) -> bool:
    snapshot = msg.get("response", {}).get("ActiveCommandsSnapshot")
    if snapshot is None:
        return False

    return any(
        command_name in command["argv"] for command in snapshot["active_commands"]
    )


async def _wait_for_active_command_state(
    subscribe: Any,
    command_name: str,
    present: bool,
    timeout: float,
) -> bool:
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        remaining = deadline - asyncio.get_running_loop().time()
        if remaining <= 0:
            return False

        try:
            msg = await asyncio.wait_for(subscribe.read_message(), timeout=remaining)
        except TimeoutError:
            return False

        if _active_commands_snapshot_has_command(msg, command_name) == present:
            return True


class LspTest(BuckTestCase):
    @buck_test()
    async def test_lsp_starts(self, buck: Buck) -> None:
        async with await buck.lsp() as lsp:
            # Will fail if the initialize response is not received
            await lsp.init_connection()

    @buck_test()
    async def test_lsp_stdin_eof_clears_server_command(
        self,
        buck: Buck,
    ) -> None:
        try:
            async with await buck.subscribe("--active-commands") as subscribe:
                lsp = await buck.lsp()
                try:
                    await lsp.init_connection()
                    assert await _wait_for_active_command_state(
                        subscribe, "lsp", present=True, timeout=10
                    )

                    assert lsp.process.stdin is not None
                    lsp.process.stdin.close()

                    exited = await _wait_for_exit(lsp.process, timeout=10)
                    assert exited
                    assert lsp.process.returncode is not None

                    assert await _wait_for_active_command_state(
                        subscribe, "lsp", present=False, timeout=10
                    )
                finally:
                    await _kill_if_alive(lsp.process)
        finally:
            await buck.kill()

    @buck_test()
    @env("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")
    async def test_lsp_exits_when_daemon_times_out(self, buck: Buck) -> None:
        # The inactivity timer only resets when a command *starts*, so it fires while the
        # lsp is still connected. The daemon cannot drain that stream, so it exits on its
        # shutdown deadline instead of waiting, and the lsp follows its daemon down.
        await buck.server()
        status = await buck.status()
        pid = json.loads(status.stdout)["process_info"]["pid"]
        daemon_dir = await buck.get_daemon_dir()
        daemon_stderr = daemon_dir / "buckd.stderr"

        lsp = await buck.lsp()
        try:
            saw_inactivity_timeout = await _wait_for_file_to_contain(
                daemon_stderr,
                "inactivity timeout elapsed",
                timeout=20,
            )
            assert saw_inactivity_timeout

            exited = await _wait_for_exit(lsp.process, timeout=20)
            assert exited
            assert not daemon_is_alive(pid)
        finally:
            await _kill_if_alive(lsp.process)

    @buck_test(skip_for_os=["windows"])
    @env("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")
    @env("BUCKD_STARTUP_TIMEOUT", "90")
    async def test_lsp_daemon_inactivity_shutdown_recovers_with_different_version(
        self,
        buck: Buck,
    ) -> None:
        # `buckd.info` outlives the daemon, and a version mismatch in it is what sends the
        # client down the connect-to-the-existing-daemon path. That used to cost the full
        # `BUCKD_STARTUP_TIMEOUT`, because the daemon accepted the connection and then
        # never answered it. Now there is nothing listening and the client replaces it.
        await buck.server()
        status = await buck.status()
        original_pid = json.loads(status.stdout)["process_info"]["pid"]
        daemon_dir = await buck.get_daemon_dir()
        daemon_stderr = daemon_dir / "buckd.stderr"
        daemon_info = daemon_dir / "buckd.info"

        lsp = await buck.lsp()
        try:
            saw_inactivity_timeout = await _wait_for_file_to_contain(
                daemon_stderr,
                "inactivity timeout elapsed",
                timeout=20,
            )
            assert saw_inactivity_timeout

            exited = await _wait_for_exit(lsp.process, timeout=20)
            assert exited
            assert not daemon_is_alive(original_pid)

            info = json.loads(daemon_info.read_text())
            info["version"] = "different-version"
            daemon_info.write_text(json.dumps(info))

            start = asyncio.get_running_loop().time()
            await buck.server()
            elapsed = asyncio.get_running_loop().time() - start

            # Well inside the 90s budget this used to consume in full.
            assert elapsed < 30, f"took {elapsed:.2f}s to recover"
            assert json.loads(daemon_info.read_text())["pid"] != original_pid
        finally:
            await _kill_if_alive(lsp.process)

    @buck_test()
    async def test_lsp_exits_when_daemon_disappears(self, buck: Buck) -> None:
        await buck.server()

        lsp = await buck.lsp()
        try:
            await lsp.init_connection()
            await buck.kill()

            exited = await _wait_for_exit(lsp.process, timeout=10)
            assert exited
            assert lsp.process.returncode is not None
        finally:
            await _kill_if_alive(lsp.process)

    @buck_test()
    @env("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")
    async def test_lsp_requests_keep_daemon_alive(self, buck: Buck) -> None:
        async with await buck.lsp() as lsp:
            await lsp.init_connection()
            daemon_info = await buck.get_daemon_dir() / "buckd.info"
            pid = json.loads(daemon_info.read_text())["pid"]

            for _ in range(6):
                await asyncio.sleep(0.2)
                await lsp.open_file(Path("clean_lint.bzl"))

            assert json.loads(daemon_info.read_text())["pid"] == pid
            assert lsp.process.returncode is None

    @buck_test(skip_for_os=["windows"])
    async def test_lsp_exits_when_daemon_is_killed(self, buck: Buck) -> None:
        await buck.server()
        status = await buck.status()
        pid = json.loads(status.stdout)["process_info"]["pid"]

        lsp = await buck.lsp()
        try:
            await lsp.init_connection()
            os.kill(pid, signal.SIGKILL)

            exited = await _wait_for_exit(lsp.process, timeout=8)
            assert exited
            assert lsp.process.returncode is not None
        finally:
            await _kill_if_alive(lsp.process)

    @buck_test()
    async def test_lints_on_open(self, buck: Buck) -> None:
        async with await buck.lsp() as lsp:
            await lsp.init_connection()
            diags = await lsp.open_file(Path("clean_lint.bzl"))
            assert diags is not None
            assert len(diags["diagnostics"]) == 0

            diags = await lsp.open_file(Path("bad_syntax.bzl"))
            assert diags is not None
            assert len(diags["diagnostics"]) == 1

    @buck_test()
    async def test_goto_definition(self, buck: Buck) -> None:
        src_targets_path = Path("dir/TARGETS.fixture")
        dest_targets_path = Path("cell/sub/TARGETS.fixture")
        dest_bzl_path = Path("cell/sub/defs.bzl")

        src_targets = fixture(buck, src_targets_path)
        dest_targets = fixture(buck, dest_targets_path)
        dest_bzl = fixture(buck, dest_bzl_path)

        async with await buck.lsp() as lsp:
            await lsp.init_connection()
            diags = await lsp.open_file(src_targets_path)
            # pyrefly: ignore [unsupported-operation]
            assert len(diags["diagnostics"]) == 0

            res = await lsp.goto_definition(
                src_targets_path,
                src_targets.start_line("load_click"),
                src_targets.start_col("load_click"),
            )
            _assert_goto_result(
                # pyrefly: ignore [bad-argument-type]
                res,
                src_targets.spans["load"],
                buck.cwd / dest_bzl_path,
                None,
            )

            res = await lsp.goto_definition(
                src_targets_path,
                src_targets.start_line("dummy_click"),
                src_targets.start_col("dummy_click"),
            )
            _assert_goto_result(
                # pyrefly: ignore [bad-argument-type]
                res,
                src_targets.spans["dummy"],
                buck.cwd / dest_bzl_path,
                dest_bzl.spans["rule"],
            )

            res = await lsp.goto_definition(
                src_targets_path,
                src_targets.start_line("missing_click"),
                src_targets.start_col("missing_click"),
            )
            # pyrefly: ignore [bad-argument-type]
            assert len(res) == 0

            res = await lsp.goto_definition(
                src_targets_path,
                src_targets.start_line("missing_foo_click"),
                src_targets.start_col("missing_foo_click"),
            )
            _assert_goto_result(
                # pyrefly: ignore [bad-argument-type]
                res,
                src_targets.spans["missing_foo"],
                buck.cwd / dest_targets_path,
                None,
            )

            res = await lsp.goto_definition(
                src_targets_path,
                src_targets.start_line("rule_click"),
                src_targets.start_col("rule_click"),
            )
            _assert_goto_result(
                # pyrefly: ignore [bad-argument-type]
                res,
                src_targets.spans["rule"],
                buck.cwd / dest_bzl_path,
                dest_bzl.spans["rule"],
            )

            res = await lsp.goto_definition(
                src_targets_path,
                src_targets.start_line("baz_click"),
                src_targets.start_col("baz_click"),
            )
            _assert_goto_result(
                # pyrefly: ignore [bad-argument-type]
                res,
                src_targets.spans["baz"],
                buck.cwd / dest_targets_path,
                dest_targets.spans["baz"],
            )

    @buck_test()
    async def test_returns_file_contents_for_starlark_types(self, buck: Buck) -> None:
        async with await buck.lsp() as lsp:
            await lsp.init_connection()

            res = await lsp.file_contents("starlark:/native/DefaultInfo.bzl")
            # pyrefly: ignore [unsupported-operation]
            assert res["contents"] is not None

            res = await lsp.file_contents("starlark:/native/NonExistent.bzl")
            # pyrefly: ignore [unsupported-operation]
            assert res["contents"] is None

            with self.assertRaises(LSPResponseError):
                await lsp.file_contents((lsp.cwd / ".buckconfig").as_uri())

    @buck_test()
    async def test_goto_definition_for_globals(self, buck: Buck) -> None:
        globals_bzl_path = Path("globals.bzl")

        globals_bzl = fixture(buck, globals_bzl_path)
        async with await buck.lsp() as lsp:
            await lsp.init_connection()
            diags = await lsp.open_file(globals_bzl_path)
            # pyrefly: ignore [unsupported-operation]
            assert len(diags["diagnostics"]) == 0

            res = await lsp.goto_definition(
                globals_bzl_path,
                globals_bzl.start_line("func2_click"),
                globals_bzl.start_col("func2_click"),
            )

            # pyrefly: ignore [bad-argument-type]
            assert len(res) == 1
            # pyrefly: ignore [unsupported-operation]
            _assert_range(res[0]["originSelectionRange"], globals_bzl.spans["func2"])
            # pyrefly: ignore [unsupported-operation]
            assert res[0]["targetRange"]["start"]["line"] != 0
            # pyrefly: ignore [unsupported-operation]
            assert res[0]["targetSelectionRange"]["start"]["line"] != 0
            _assert_uris(
                # pyrefly: ignore [unsupported-operation]
                res[0]["targetUri"],
                (buck.cwd / "prelude" / "prelude.bzl").as_uri(),
            )

            res = await lsp.goto_definition(
                globals_bzl_path,
                globals_bzl.start_line("info_click"),
                globals_bzl.start_col("info_click"),
            )

            # pyrefly: ignore [bad-argument-type]
            assert len(res) == 1
            # pyrefly: ignore [unsupported-operation]
            _assert_range(res[0]["originSelectionRange"], globals_bzl.spans["info"])
            # pyrefly: ignore [unsupported-operation]
            _assert_uris(res[0]["targetUri"], "starlark:/native/DefaultInfo.bzl")

            res = await lsp.goto_definition(
                globals_bzl_path,
                globals_bzl.start_line("invalid_click"),
                globals_bzl.start_col("invalid_click"),
            )
            # pyrefly: ignore [bad-argument-type]
            assert len(res) == 0

    @buck_test()
    async def test_supports_bxl_files(self, buck: Buck) -> None:
        src_bxl_path = Path("query.bxl")

        src_bxl = fixture(buck, src_bxl_path)

        async with await buck.lsp() as lsp:
            await lsp.init_connection()
            diags = await lsp.open_file(src_bxl_path)
            # pyrefly: ignore [unsupported-operation]
            assert len(diags["diagnostics"]) == 0

            res = await lsp.goto_definition(
                src_bxl_path,
                src_bxl.start_line("foo_click"),
                src_bxl.start_col("foo_click"),
            )
            _assert_goto_result(
                # pyrefly: ignore [bad-argument-type]
                res,
                src_bxl.spans["foo"],
                buck.cwd / src_bxl_path,
                src_bxl.spans["dest_foo"],
            )

            res = await lsp.goto_definition(
                src_bxl_path,
                src_bxl.start_line("f_click"),
                src_bxl.start_col("f_click"),
            )
            _assert_goto_result(
                # pyrefly: ignore [bad-argument-type]
                res,
                src_bxl.spans["f"],
                buck.cwd / src_bxl_path,
                src_bxl.spans["dest_f"],
            )

    @buck_test()
    async def test_goto_definition_after_non_ascii_text(self, buck: Buck) -> None:
        # The LSP column counts UTF-16 units; `x` is at column 12 after a three-character
        # CJK string, at byte 18.
        path = Path("utf16_column.bzl")
        async with await buck.lsp() as lsp:
            await lsp.init_connection()
            await lsp.open_file(path)
            res = await lsp.goto_definition(path, 9, 12)
            # pyrefly: ignore [unsupported-operation]
            assert res[0]["targetRange"] == {
                "start": {"line": 8, "character": 0},
                "end": {"line": 8, "character": 1},
            }, res
