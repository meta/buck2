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
import os
import shutil
import sys
from pathlib import PurePosixPath

import unittest # @oss-enable

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class AgentContextTest(BuckTestCase):
    @buck_test()
    async def test_agent_context_valid_non_enforced(self, buck: Buck) -> None:
        """Non-enforced client with valid agent context should succeed."""
        await buck.build(
            "//:pass",
            "--agent-context",
            "intent=build,attempt=1",
        )

    @buck_test()
    async def test_agent_context_non_enforced_no_validation(self, buck: Buck) -> None:
        """Non-enforced client with invalid values should still succeed (stored as-is)."""
        await buck.build(
            "//:pass",
            "--agent-context",
            "intent=garbage_value,attempt=1",
        )

    @buck_test()
    async def test_agent_context_enforced_valid(self, buck: Buck) -> None:
        """Enforced client with all required fields and valid values should succeed."""
        await buck.build(
            "//:pass",
            "--client-metadata",
            "id=test_enforced_client",
            "--agent-context",
            "intent=build,attempt=1",
        )

    @buck_test()
    async def test_agent_context_enforced_with_optional_field(self, buck: Buck) -> None:
        """Enforced client with required + optional fields should succeed."""
        await buck.build(
            "//:pass",
            "--client-metadata",
            "id=test_enforced_client",
            "--agent-context",
            "intent=fix,attempt=2,prior_error=missing_target",
        )

    @buck_test()
    async def test_agent_context_enforced_invalid_value(self, buck: Buck) -> None:
        """Enforced client with invalid value for constrained field should fail
        with the exact error message including valid values and description."""
        await expect_failure(
            buck.build(
                "//:pass",
                "--client-metadata",
                "id=test_enforced_client",
                "--agent-context",
                "intent=invalid_value,attempt=1",
            ),
            stderr_regex=(
                r"Invalid agent-context value `invalid_value` for key `intent`\."
                r"\s+intent: The purpose of this buck2 invocation"
                r"\s+Valid values: build, test, query, fix, investigate"
            ),
        )

    @buck_test()
    async def test_agent_context_enforced_unknown_key(self, buck: Buck) -> None:
        """Enforced client with unknown key should fail with sorted list of valid keys."""
        await expect_failure(
            buck.build(
                "//:pass",
                "--client-metadata",
                "id=test_enforced_client",
                "--agent-context",
                "intent=build,attempt=1,unknown_key=foo",
            ),
            stderr_regex=(
                r"Unknown agent-context key `unknown_key`\."
                r"\s+Valid keys: attempt, intent, prior_error"
            ),
        )

    @buck_test()
    async def test_agent_context_enforced_missing_required(self, buck: Buck) -> None:
        """Enforced client missing required fields should list all missing fields sorted."""
        await expect_failure(
            buck.build(
                "//:pass",
                "--client-metadata",
                "id=test_enforced_client",
                "--agent-context",
                "prior_error=missing_target",
            ),
            stderr_regex=(
                r"Missing required agent-context field\(s\):"
                r"\s+- attempt: Which attempt number this is for the same logical task"
                r"\s+- intent: The purpose of this buck2 invocation"
            ),
        )

    @buck_test()
    async def test_agent_context_enforced_empty_value_counts_as_missing(
        self,
        buck: Buck,
    ) -> None:
        """Enforced client with empty value for required field should report it as missing."""
        await expect_failure(
            buck.build(
                "//:pass",
                "--client-metadata",
                "id=test_enforced_client",
                "--agent-context",
                "intent=,attempt=1",
            ),
            stderr_regex=(
                r"Missing required agent-context field\(s\):"
                r"\s+- intent: The purpose of this buck2 invocation"
            ),
        )

    @buck_test()
    async def test_agent_context_no_context_passes(self, buck: Buck) -> None:
        """Build without --agent-context should always succeed."""
        await buck.build("//:pass")

    @buck_test()
    async def test_agent_context_enforced_no_context_passes(self, buck: Buck) -> None:
        """Enforced client without --agent-context should succeed (not required in v1)."""
        await buck.build(
            "//:pass",
            "--client-metadata",
            "id=test_enforced_client",
        )

    @buck_test()
    async def test_agent_context_invalid_format(self, buck: Buck) -> None:
        """Malformed --agent-context should fail with the exact format error."""
        await expect_failure(
            buck.build(
                "//:pass",
                "--agent-context",
                "not_a_valid_format",
            ),
            stderr_regex=(
                r"Invalid agent-context format: `not_a_valid_format`\."
                r" Each entry must be a `key=value` pair\."
            ),
        )

    @buck_test()
    async def test_agent_context_invalid_key_format(self, buck: Buck) -> None:
        """Non-snake_case key should fail with the exact key format error."""
        await expect_failure(
            buck.build(
                "//:pass",
                "--agent-context",
                "InvalidKey=value",
            ),
            stderr_regex=(
                r"Invalid agent-context key: `InvalidKey`\."
                r" Keys must be snake_case identifiers\."
            ),
        )

    @buck_test(write_invocation_record=True)
    async def test_agent_context_argfile_before_subcommand(self, buck: Buck) -> None:
        (buck.cwd / "context.args").write_text("--agent-context=intent=build\n")
        result = await buck.run_buck_command(
            "@context.args",
            "--client-metadata",
            "id=test_enforced_client",
            "build",
            "//:pass",
            "--agent-context",
            "attempt=1",
        )
        context = result.invocation_record()["agent_context"]
        assert [
            entry for entry in context if entry["key"] in {"intent", "attempt"}
        ] == [
            {"key": "intent", "value": "build"},
            {"key": "attempt", "value": "1"},
        ]

    @buck_test(write_invocation_record=True)
    async def test_agent_context_logged_to_invocation_record(self, buck: Buck) -> None:
        """Agent context should appear in the invocation record."""
        res = await buck.build(
            "//:pass",
            "--agent-context",
            "intent=build,attempt=1",
        )

        record = res.invocation_record()
        agent_context = record.get("agent_context")
        assert agent_context is not None, (
            "agent_context should be present in invocation record"
        )

        entry_map = {e["key"]: e["value"] for e in agent_context}
        assert entry_map["intent"] == "build"
        assert entry_map["attempt"] == "1"
        assert record.get("agent_direct_call") is None

    @buck_test(write_invocation_record=True)
    async def test_agent_direct_call_logged_to_invocation_record(
        self, buck: Buck
    ) -> None:
        for value, expected in [("true", True), ("false", False), ("invalid", None)]:
            res = await buck.build(
                "//:pass",
                "--client-metadata",
                "id=test_enforced_client",
                "--agent-context",
                f"intent=build,attempt=1,direct_call={value}",
            )
            record = res.invocation_record()
            assert record.get("agent_direct_call") is expected
            assert {"key": "direct_call", "value": value} in record["agent_context"]

    @buck_test(write_invocation_record=True)
    async def test_agent_direct_call_last_value_wins(self, buck: Buck) -> None:
        for value, expected in [("false", False), ("invalid", None)]:
            res = await buck.build(
                "//:pass",
                "--agent-context",
                "direct_call=true",
                "--agent-context",
                f"direct_call={value}",
            )
            assert res.invocation_record().get("agent_direct_call") is expected

    @buck_test(write_invocation_record=True)
    async def test_agent_direct_call_alone_does_not_require_context(
        self, buck: Buck
    ) -> None:
        res = await buck.build(
            "//:pass",
            "--client-metadata",
            "id=test_enforced_client",
            "--agent-context",
            "direct_call=true",
        )
        assert res.invocation_record()["agent_direct_call"] is True

    # Ancestry inference is enabled only on Linux.
    @unittest.skip("Ancestry inference requires a Meta internal build") # @oss-enable
    @buck_test(skip_for_os=["darwin", "windows"])
    async def test_detect_direct_agent_and_stop_at_tools(self, buck: Buck) -> None:
        # Prost serializes protobuf enum fields as integers in invocation-record JSON.
        caller_agent, caller_command_shell, caller_other = 1, 2, 3
        caller_command_wrapper = 5
        stop_agent, stop_other = 1, 2
        buck._env.pop("CODING_AGENT_METADATA", None)
        # Copy a shell so its executable identity is stable without depending on an
        # installed agent or an LLM service. Keep it alive while Buck reads its parent.
        agent = buck.cwd / "codex"
        shutil.copy2("/bin/sh", agent)
        script = buck.cwd / "tool.sh"
        script.write_text('"$@"\nexit $?\n')
        python_tool = buck.cwd / "tool.py"
        python_tool.write_text(
            "import subprocess, sys\nsys.exit(subprocess.call(sys.argv[1:]))\n"
        )
        exec_script = buck.cwd / "exec_tool.sh"
        exec_script.write_text('exec "$@"\n')
        record_path = buck.cwd / "record.json"

        async def run(prefix: list[str], *extra: str) -> tuple[int, str, dict]:
            command = buck.construct_buck_command(
                "build",
                "//:pass",
                "--unstable-write-invocation-record",
                str(record_path),
                *extra,
            )
            process = await asyncio.create_subprocess_exec(
                str(agent),
                "-c",
                '"$@"; exit $?',
                "agent",
                *prefix,
                *command,
                cwd=buck.cwd,
                env=buck._get_command_env(
                    {"CODING_AGENT_METADATA": "id=codex,direct_call=true"}
                ),
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            _, stderr = await asyncio.wait_for(process.communicate(), timeout=120)
            assert process.returncode is not None
            return (
                process.returncode,
                stderr.decode(),
                json.loads(record_path.read_text())["data"]["Record"]["data"][
                    "InvocationRecord"
                ],
            )

        # Reuse an agent-started daemon from tools and the test runner. Only the
        # client's ancestry contributes to inference; it does not change the explicit marker.
        direct = ["/bin/sh", "-c", '"$@"; exit $?', "shell"]
        code, stderr, record = await run(direct)
        assert code == 0, stderr
        assert record.get("agent_direct_call") is None
        assert record["agent_inferred_direct_call"] is True
        assert record["agent_context"] == [{"key": "id", "value": "codex"}]
        shell_name = PurePosixPath(os.path.realpath("/bin/sh")).name
        assert record["agent_callers"] == [
            {"kind": caller_command_shell, "executable": shell_name},
            {"kind": caller_agent, "executable": "codex"},
        ]
        assert record["agent_detection_stop_reason"] == stop_agent
        daemon = json.loads((await buck.status()).stdout)["process_info"]["pid"]

        tools = [["/bin/sh", str(script)], [sys.executable, str(python_tool)]]
        for prefix in [direct, *tools]:
            for value in [None, "true", "false", "invalid"]:
                extra = (
                    [] if value is None else ["--agent-context", f"direct_call={value}"]
                )
                code, stderr, record = await run(prefix, *extra)
                assert code == 0, stderr
                assert record.get("agent_direct_call") is (
                    None if value in [None, "invalid"] else value == "true"
                )
                assert record["agent_inferred_direct_call"] is (prefix == direct)
                if prefix == direct:
                    assert record["agent_detection_stop_reason"] == stop_agent
                    assert record["agent_callers"][-1] == {
                        "kind": caller_agent,
                        "executable": "codex",
                    }
                else:
                    assert record["agent_detection_stop_reason"] == stop_other
                    assert record["agent_callers"] == [
                        {
                            "kind": caller_other,
                            "executable": PurePosixPath(
                                os.path.realpath(prefix[0])
                            ).name,
                        }
                    ]

        timeout = ["/usr/bin/timeout", "60"]
        timeout_caller = {"kind": caller_command_wrapper, "executable": "timeout"}
        for prefix in [timeout, [*timeout, *direct], [*direct, *timeout]]:
            code, stderr, record = await run(prefix)
            assert code == 0, stderr
            assert record.get("agent_direct_call") is None
            assert record["agent_inferred_direct_call"] is True
            assert record["agent_detection_stop_reason"] == stop_agent
            assert record["agent_callers"].count(timeout_caller) == 1
            assert record["agent_callers"][-1] == {
                "kind": caller_agent,
                "executable": "codex",
            }

        # timeout must not hide a script/tool boundary on either side of it.
        for tool in tools:
            tool_caller = {
                "kind": caller_other,
                "executable": PurePosixPath(os.path.realpath(tool[0])).name,
            }
            for prefix, expected_callers in [
                ([*timeout, *tool], [tool_caller]),
                ([*tool, *timeout], [timeout_caller, tool_caller]),
            ]:
                code, stderr, record = await run(prefix)
                assert code == 0, stderr
                assert record.get("agent_direct_call") is None
                assert record["agent_inferred_direct_call"] is False
                assert record["agent_detection_stop_reason"] == stop_other
                assert record["agent_callers"] == expected_callers

        # exec and sourced scripts erase the visible tool boundary. These known
        # false positives must leave the explicit direct-call marker unset.
        for prefix in [
            ["/bin/sh", str(exec_script)],
            ["/bin/sh", "-c", f'. "{script}"', "shell"],
        ]:
            code, stderr, record = await run(prefix)
            assert code == 0, stderr
            assert record.get("agent_direct_call") is None
            assert record["agent_inferred_direct_call"] is True

        code, stderr, record = await run(
            [],
            "--agent-context",
            "direct_call=true",
            "--agent-context",
            "advice_ack=test_advice",
        )
        assert code == 0, stderr
        assert record["agent_direct_call"] is True
        assert record["agent_inferred_direct_call"] is True
        await buck.build(
            "//:pass",
            "--unstable-write-invocation-record",
            str(record_path),
        )
        record = json.loads(record_path.read_text())["data"]["Record"]["data"][
            "InvocationRecord"
        ]
        assert record["agent_inferred_direct_call"] is False
        assert record["agent_detection_stop_reason"] == stop_other
        assert record["agent_callers"][-1]["kind"] == caller_other
        assert record.get("agent_direct_call") is None
        assert not record.get("agent_context")
        assert json.loads((await buck.status()).stdout)["process_info"]["pid"] == daemon
