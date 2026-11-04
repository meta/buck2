# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


ENABLED = ("--setting=agent_config_change.enabled=true",)
DIRECT = "--agent-context=direct_call=true"
ADVICE = r"Buck advice \[config_change\]"
EXPLICIT_TAG = "agent_interactive.config_change.explicit"
ENABLED_TAG = "agent_interactive.config_change.enabled"
DISABLED_TAG = "agent_interactive.config_change.disabled"


class AgentConfigChangeTest(BuckTestCase):
    @buck_test(write_invocation_record=True)
    async def test_dropping_config_args_is_blocked_and_keeps_daemon_state(
        self, buck: Buck
    ) -> None:
        await buck.targets("//:", "-c", "foo.bar=1", *ENABLED, DIRECT)

        # Like `uquery` without the preceding build's flags.
        result = await expect_failure(
            buck.targets("//:", *ENABLED, DIRECT),
            stderr_regex=ADVICE,
        )
        assert "that this command drops:" in result.stderr
        assert "--config foo.bar=1" in result.stderr
        record = result.invocation_record()
        assert "agent_interactive.config_change.block" in record["tags"]
        assert ENABLED_TAG in record["tags"]
        assert "AGENT_ADVICE_BLOCKED" in record.single_error()["tags"]

        # The blocked command never committed, so returning to the original config finds the
        # daemon's state as the first command left it.
        after = await buck.targets("//:", "-c", "foo.bar=1", *ENABLED, DIRECT)
        record = after.invocation_record()
        assert record.get("previous_uuid_with_mismatched_config") is None
        assert record["new_configs_used"] == 0

    @buck_test()
    async def test_dropping_some_config_args_is_blocked(self, buck: Buck) -> None:
        await buck.targets(
            "//:", "-c", "foo.bar=1", "-c", "foo.baz=2", *ENABLED, DIRECT
        )
        result = await expect_failure(
            buck.targets("//:", "-c", "foo.bar=1", *ENABLED, DIRECT),
            stderr_regex=ADVICE,
        )
        assert "--config foo.baz=2" in result.stderr
        assert "--config foo.bar=1" not in result.stderr

    @buck_test(write_invocation_record=True)
    async def test_explicit_config_changes_are_allowed_and_tagged(
        self, buck: Buck
    ) -> None:
        await buck.targets("//:", "-c", "foo.bar=1", *ENABLED, DIRECT)
        for args in [
            # A different value for the same key.
            ("-c", "foo.bar=2"),
            # An additional key.
            ("-c", "foo.bar=2", "-c", "foo.baz=3"),
        ]:
            result = await buck.targets("//:", *args, *ENABLED, DIRECT)
            assert "Buck advice" not in result.stderr
            assert EXPLICIT_TAG in result.invocation_record()["tags"]

    @buck_test(write_invocation_record=True)
    async def test_dropping_args_that_restate_repo_values_is_allowed(
        self, buck: Buck
    ) -> None:
        # The fixture's `.buckconfig` already sets `test.default = 2`.
        await buck.targets("//:", "-c", "test.default=2", *ENABLED, DIRECT)
        result = await buck.targets("//:", *ENABLED, DIRECT)
        assert "Buck advice" not in result.stderr
        assert EXPLICIT_TAG not in result.invocation_record()["tags"]

    @buck_test(write_invocation_record=True)
    async def test_drop_requires_its_own_ack_and_nonempty_reason(
        self, buck: Buck
    ) -> None:
        await buck.targets("//:", "-c", "foo.bar=1", *ENABLED, DIRECT)
        for context in [
            "advice_ack=killall,reason=Needs the repo defaults",
            "advice_ack=config_change",
            "advice_ack=config_change,reason=   ",
            "reason=Needs the repo defaults",
        ]:
            await expect_failure(
                buck.targets("//:", *ENABLED, DIRECT, f"--agent-context={context}"),
                stderr_regex=ADVICE,
            )

        result = await buck.targets(
            "//:",
            *ENABLED,
            DIRECT,
            "--agent-context=advice_ack=config_change,reason=Needs the repo defaults",
        )
        record = result.invocation_record()
        assert "agent_interactive.config_change.ack" in record["tags"]
        assert record["previous_uuid_with_mismatched_config"]

    @buck_test()
    async def test_unchanged_config_is_not_blocked(self, buck: Buck) -> None:
        # The first command after daemon start has no state to invalidate.
        for args in [
            ("-c", "foo.bar=1"),
            ("-c", "foo.bar=1"),
            # Keeps the daemon's configuration even though the command drops `-c foo.bar=1`.
            ("--reuse-current-config",),
        ]:
            result = await buck.targets("//:", *args, *ENABLED, DIRECT)
            assert "Buck advice" not in result.stderr

    @buck_test(write_invocation_record=True)
    async def test_direct_calls_record_whether_the_guard_is_on(
        self, buck: Buck
    ) -> None:
        for args, expected in [
            ((*ENABLED, DIRECT), ENABLED_TAG),
            ((DIRECT,), DISABLED_TAG),
            (("--setting=agent_config_change.enabled=false", DIRECT), DISABLED_TAG),
        ]:
            tags = (await buck.targets("//:", *args)).invocation_record()["tags"]
            assert expected in tags, args
            assert ({ENABLED_TAG, DISABLED_TAG} - {expected}).isdisjoint(tags), args

        # Only direct agent calls are in the experiment population.
        for args in [(*ENABLED,), (*ENABLED, "--agent-context=direct_call=false")]:
            tags = (await buck.targets("//:", *args)).invocation_record()["tags"]
            assert ENABLED_TAG not in tags and DISABLED_TAG not in tags, args

    @buck_test()
    async def test_master_switch_does_not_affect_the_guard(self, buck: Buck) -> None:
        for master in ["true", "false"]:
            await buck.targets("//:", "-c", "foo.bar=1", *ENABLED, DIRECT)
            await expect_failure(
                buck.targets(
                    "//:",
                    f"--setting=agent_interactive.enabled={master}",
                    *ENABLED,
                    DIRECT,
                ),
                stderr_regex=ADVICE,
            )

    @buck_test()
    async def test_disabled_and_non_direct_calls_keep_existing_behavior(
        self, buck: Buck
    ) -> None:
        for args in [
            (DIRECT,),
            # The master switch alone does not turn on this guard.
            ("--setting=agent_interactive.enabled=true", DIRECT),
            (
                *ENABLED,
                "--setting=agent_config_change.enabled=false",
                DIRECT,
            ),
            (*ENABLED,),
            (*ENABLED, "--agent-context=direct_call=false"),
        ]:
            for config in [("-c", "foo.bar=1"), ()]:
                result = await buck.targets("//:", *config, *args)
                assert "Buck advice" not in result.stderr
