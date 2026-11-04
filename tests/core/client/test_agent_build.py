# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import json

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


ENABLED = "--setting=agent_build.enabled=true"
DIRECT = "--agent-context=direct_call=true"
ACK = "--agent-context=advice_ack=build,reason=Check code generation"
ADVICE = r"Buck advice \[build\]"


class AgentBuildTest(BuckTestCase):
    @buck_test(write_invocation_record=True)
    async def test_language_messages_and_rule_tags(self, buck: Buck) -> None:
        for target, rule_type, language, message, absent in [
            ("rust", "rust_binary", "rust", "arc rust-check root//:rust", "C++"),
            (
                "rust_library",
                "rust_library",
                "rust",
                "arc rust-check root//:rust_library",
                "Rust binary",
            ),
            ("cpp", "cxx_library", "cxx", "arc cpp-check root//:cpp", "arc rust-check"),
            (
                "cpp_binary",
                "cxx_binary",
                "cxx",
                "arc cpp-check root//:cpp_binary",
                "arc rust-check",
            ),
            (
                "rust_test",
                "rust_test",
                "rust",
                "arc rust-check root//:rust_test",
                "arc cpp-check",
            ),
            (
                "cpp_test",
                "cxx_test",
                "cxx",
                "arc cpp-check root//:cpp_test",
                "arc rust-check",
            ),
            (
                "rust_custom",
                "rust_custom",
                "rust",
                "arc rust-check root//:rust_custom",
                "arc cpp-check",
            ),
            (
                "cpp_custom",
                "cxx_custom",
                "cxx",
                "arc cpp-check root//:cpp_custom",
                "arc rust-check",
            ),
        ]:
            result = await expect_failure(
                buck.build(f"root//:{target}", ENABLED, DIRECT), stderr_regex=ADVICE
            )
            assert f"rule type `{rule_type}`" in result.stderr
            assert message in result.stderr
            assert absent not in result.stderr
            assert "[check]" not in result.stderr
            assert "If your goal is to verify" in result.stderr
            assert (
                "If your goal is to run tests, use `buck2 test` directly"
                in result.stderr
            )
            assert "a separate build step is unnecessary" in result.stderr
            record = result.invocation_record()
            assert {
                tag
                for tag in record["tags"]
                if tag.startswith("agent_interactive.build.")
            } == {
                "agent_interactive.build.enabled",
                f"agent_interactive.build.{language}.enabled",
                f"agent_interactive.build.{rule_type}.enabled",
                "agent_interactive.build.block",
                f"agent_interactive.build.{language}.block",
                f"agent_interactive.build.{rule_type}.block",
            }
            assert "AGENT_ADVICE_BLOCKED" in record.single_error()["tags"]
            acknowledged = await buck.build(f"root//:{target}", ENABLED, DIRECT, ACK)
            assert {
                tag
                for tag in acknowledged.invocation_record()["tags"]
                if tag.startswith("agent_interactive.build.")
            } == {
                "agent_interactive.build.enabled",
                f"agent_interactive.build.{language}.enabled",
                f"agent_interactive.build.{rule_type}.enabled",
                "agent_interactive.build.ack",
                f"agent_interactive.build.{language}.ack",
                f"agent_interactive.build.{rule_type}.ack",
            }
            disabled = await buck.build(
                f"root//:{target}",
                ENABLED,
                "--setting=agent_build.enabled=false",
                DIRECT,
                ACK,
            )
            assert {
                tag
                for tag in disabled.invocation_record()["tags"]
                if tag.startswith("agent_interactive.build.")
            } == {
                "agent_interactive.build.disabled",
                f"agent_interactive.build.{language}.disabled",
                f"agent_interactive.build.{rule_type}.disabled",
            }

    @buck_test(write_invocation_record=True)
    async def test_blocks_before_build_actions(self, buck: Buck) -> None:
        await expect_failure(
            buck.build("root//:fails", ENABLED, DIRECT), stderr_regex=ADVICE
        )
        await expect_failure(
            buck.build("root//:fails", ENABLED, DIRECT, ACK),
            stderr_regex="build action executed",
        )
        disabled = await expect_failure(
            buck.build("root//:fails", DIRECT),
            stderr_regex="build action executed",
        )
        assert "Buck advice" not in disabled.stderr
        assert {
            tag
            for tag in disabled.invocation_record()["tags"]
            if tag.startswith("agent_interactive.build.")
        } == {
            "agent_interactive.build.disabled",
            "agent_interactive.build.rust.disabled",
            "agent_interactive.build.rust_binary.disabled",
        }

    @buck_test(write_invocation_record=True)
    async def test_unrelated_rules_and_subtargets_proceed(self, buck: Buck) -> None:
        for target in [
            "plain",
            "rust_library[check]",
            "rust[check]",
            "cpp[check]",
            "cpp[custom]",
        ]:
            for settings in [(ENABLED,), ()]:
                result = await buck.build(f"root//:{target}", *settings, DIRECT)
                assert "Buck advice" not in result.stderr
                assert not any(
                    tag.startswith("agent_interactive.build.")
                    for tag in result.invocation_record()["tags"]
                )

    @buck_test()
    async def test_rule_filter_applies_in_target_universe(self, buck: Buck) -> None:
        await expect_failure(
            buck.build("root//:cpp", "--target-universe=root//:cpp", ENABLED, DIRECT),
            stderr_regex="arc cpp-check root//:cpp",
        )
        result = await buck.build(
            "root//:plain", "--target-universe=root//:plain", ENABLED, DIRECT
        )
        assert "Buck advice" not in result.stderr

    @buck_test(write_invocation_record=True)
    async def test_acknowledgement_applies_to_one_build(self, buck: Buck) -> None:
        for context in [
            "advice_ack=build_intent,reason=Check code generation",
            "advice_ack=clean,reason=Check code generation",
            "advice_ack=build",
            "advice_ack=build,reason=   ",
            "advice_ack=build,reason=old,reason=",
            "reason=Check code generation",
        ]:
            await expect_failure(
                buck.build(
                    "root//:rust", ENABLED, DIRECT, f"--agent-context={context}"
                ),
                stderr_regex=ADVICE,
            )
        for target in ["rust", "rust_library"]:
            result = await buck.build(f"root//:{target}", ENABLED, DIRECT, ACK)
            assert "agent_interactive.build.ack" in result.invocation_record()["tags"]
            await expect_failure(
                buck.build(f"root//:{target}", ENABLED, DIRECT), stderr_regex=ADVICE
            )

    @buck_test(write_invocation_record=True)
    async def test_mixed_targets_only_block_matching_rules(self, buck: Buck) -> None:
        report_path = buck.cwd / "report.json"
        result = await expect_failure(
            buck.build(
                "root//:cpp",
                "root//:rust_test",
                "root//:plain",
                ENABLED,
                DIRECT,
                "--build-report",
                str(report_path),
            ),
            stderr_regex=ADVICE,
        )
        results = json.loads(report_path.read_text())["results"]
        assert results["root//:cpp"]["success"] == "FAIL"
        assert results["root//:rust_test"]["success"] == "FAIL"
        assert results["root//:plain"]["success"] == "SUCCESS"
        assert {
            tag
            for tag in result.invocation_record()["tags"]
            if tag.startswith("agent_interactive.build.")
        } == {
            "agent_interactive.build.enabled",
            "agent_interactive.build.cxx.enabled",
            "agent_interactive.build.rust.enabled",
            "agent_interactive.build.cxx_library.enabled",
            "agent_interactive.build.rust_test.enabled",
            "agent_interactive.build.block",
            "agent_interactive.build.cxx.block",
            "agent_interactive.build.rust.block",
            "agent_interactive.build.cxx_library.block",
            "agent_interactive.build.rust_test.block",
        }
        disabled = await buck.build(
            "root//:cpp", "root//:rust_test", "root//:plain", DIRECT
        )
        assert {
            tag
            for tag in disabled.invocation_record()["tags"]
            if tag.startswith("agent_interactive.build.")
        } == {
            "agent_interactive.build.disabled",
            "agent_interactive.build.cxx.disabled",
            "agent_interactive.build.rust.disabled",
            "agent_interactive.build.cxx_library.disabled",
            "agent_interactive.build.rust_test.disabled",
        }

    @buck_test(write_invocation_record=True)
    async def test_explicit_artifact_requests_proceed(self, buck: Buck) -> None:
        for args in [
            ("--show-output",),
            ("--show-full-output",),
            ("--show-json-output",),
            ("--show-full-json-output",),
            ("--materializations=all",),
            ("--out", str(buck.cwd / "output.txt")),
        ]:
            for settings in [(ENABLED,), ()]:
                result = await buck.build("root//:rust", *settings, DIRECT, *args)
                assert "Buck advice" not in result.stderr
                assert not any(
                    tag.startswith("agent_interactive.build.")
                    for tag in result.invocation_record()["tags"]
                )
        assert (buck.cwd / "output.txt").read_text() == "pass"
        await expect_failure(
            buck.build("root//:rust", ENABLED, DIRECT, "--materializations=none"),
            stderr_regex=ADVICE,
        )

    @buck_test(write_invocation_record=True)
    async def test_disabled_and_non_direct_builds_proceed(self, buck: Buck) -> None:
        for args in [
            (DIRECT,),
            ("--setting=agent_interactive.enabled=true", DIRECT),
            (ENABLED, "--setting=agent_build.enabled=false", DIRECT),
            (
                "--setting=agent_interactive.enabled=true",
                "--setting=agent_build.enabled=false",
                DIRECT,
            ),
        ]:
            result = await buck.build("root//:rust", *args)
            assert "Buck advice" not in result.stderr
            assert {
                tag
                for tag in result.invocation_record()["tags"]
                if tag.startswith("agent_interactive.build.")
            } == {
                "agent_interactive.build.disabled",
                "agent_interactive.build.rust.disabled",
                "agent_interactive.build.rust_binary.disabled",
            }
        for args in [
            (ENABLED,),
            (ENABLED, DIRECT, "--agent-context=direct_call=false"),
            (DIRECT, "--agent-context=direct_call=false"),
        ]:
            result = await buck.build("root//:rust", *args)
            assert "Buck advice" not in result.stderr
            assert not any(
                tag.startswith("agent_interactive.build.")
                for tag in result.invocation_record()["tags"]
            )
        result = await buck.build(ENABLED, DIRECT)
        assert "NO BUILD TARGET PATTERNS SPECIFIED" in result.stderr
        assert "Buck advice" not in result.stderr
        assert not any(
            tag.startswith("agent_interactive.build.")
            for tag in result.invocation_record()["tags"]
        )

    @buck_test(write_invocation_record=True)
    async def test_build_setting_is_independent(self, buck: Buck) -> None:
        for interactive in ["false", "true"]:
            result = await expect_failure(
                buck.build(
                    "root//:rust",
                    ENABLED,
                    DIRECT,
                    f"--setting=agent_interactive.enabled={interactive}",
                ),
                stderr_regex=ADVICE,
            )
            assert (
                "agent_interactive.build.rust.enabled"
                in result.invocation_record()["tags"]
            )

    @buck_test(write_invocation_record=True)
    async def test_testing_and_running_do_not_require_build_ack(
        self, buck: Buck
    ) -> None:
        tested = await buck.test("root//:rust_test", "root//:cpp_test", ENABLED, DIRECT)
        assert "Buck advice" not in tested.stderr
        ran = await buck.run("root//:rust", ENABLED, DIRECT)
        assert "Buck advice" not in ran.stderr
        for result in [tested, ran]:
            assert not any(
                tag.startswith("agent_interactive.build.")
                for tag in result.invocation_record()["tags"]
            )
