# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import json
import uuid

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import ExitCodeV2
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


ENABLED = (
    "--setting=agent_interactive.enabled=true",
    "--setting=agent_interactive.new_isolation_dir_enabled=true",
)
DIRECT = "--agent-context=direct_call=true"


class AgentIsolationTest(BuckTestCase):
    @buck_test()
    async def test_client_settings_reuse_existing_daemon(self, buck: Buck) -> None:
        await buck.audit_config()
        daemon_id = json.loads((await buck.status()).stdout)["daemon_constraints"][
            "daemon_id"
        ]
        for args in [(*ENABLED, DIRECT), (DIRECT,)]:
            await buck.audit_config(*args)
            assert (
                json.loads((await buck.status()).stdout)["daemon_constraints"][
                    "daemon_id"
                ]
                == daemon_id
            )

    @buck_test(write_invocation_record=True)
    async def test_new_isolation_is_blocked_until_acknowledged(
        self, buck: Buck
    ) -> None:
        buck.set_isolation_prefix(uuid.uuid4().hex)
        # Status may create the daemon directory, but that does not initialize an isolation.
        assert "no buckd running" in (await buck.status()).stderr
        for context in [
            "reason=Inspect config",
            "advice_ack=killall,reason=Inspect config",
            "advice_ack=clean,reason=Inspect config",
            "advice_ack=new_isolation_dir",
            "advice_ack=new_isolation_dir,reason=   ",
        ]:
            result = await expect_failure(
                buck.audit_config(*ENABLED, DIRECT, f"--agent-context={context}"),
                exit_code=ExitCodeV2.USER_ERROR,
                stderr_regex=r"Buck advice \[new_isolation_dir\]",
            )
            assert "Starting new buck2 daemon" not in result.stderr
            assert "Failed to connect to buck daemon" not in result.stderr
            assert "no buckd running" in (await buck.status()).stderr
            record = result.invocation_record()
            assert "agent_interactive.new_isolation_dir.block" in record["tags"]
            assert "agent_interactive.new_isolation_dir.enabled" in record["tags"]
            assert "AGENT_ADVICE_BLOCKED" in record.single_error()["tags"]

        result = await buck.audit_config(
            *ENABLED,
            DIRECT,
            "--agent-context=advice_ack=new_isolation_dir,reason=Use isolated test state",
        )
        assert (
            "agent_interactive.new_isolation_dir.ack"
            in result.invocation_record()["tags"]
        )
        before = json.loads((await buck.status()).stdout)["process_info"]["pid"]
        reused = await buck.audit_config(*ENABLED, DIRECT)
        assert json.loads((await buck.status()).stdout)["process_info"]["pid"] == before
        assert (
            "agent_interactive.new_isolation_dir.ack"
            not in reused.invocation_record()["tags"]
        )
        await buck.kill()
        # Restarting a previously initialized isolation also reuses its stored state.
        await buck.audit_config(*ENABLED, DIRECT)

    @buck_test(write_invocation_record=True)
    async def test_disabled_direct_new_isolation_records_its_state(
        self, buck: Buck
    ) -> None:
        buck.set_isolation_prefix(uuid.uuid4().hex)
        result = await buck.audit_config(DIRECT)
        assert "Buck advice" not in result.stderr
        assert (
            "agent_interactive.new_isolation_dir.disabled"
            in result.invocation_record()["tags"]
        )

        # Reusing the isolation's daemon does not start a new isolation.
        reused = await buck.audit_config(DIRECT)
        assert (
            "agent_interactive.new_isolation_dir.disabled"
            not in reused.invocation_record()["tags"]
        )

    @buck_test()
    async def test_default_isolation_needs_no_ack(self, buck: Buck) -> None:
        buck.set_isolation_prefix("v2")
        result = await buck.audit_config(*ENABLED, DIRECT)
        assert "Buck advice" not in result.stderr

    @buck_test()
    async def test_disabled_and_non_direct_isolation_keep_existing_behavior(
        self,
        buck: Buck,
    ) -> None:
        buck.set_isolation_prefix(uuid.uuid4().hex)
        for args in [
            (DIRECT,),
            (
                *ENABLED,
                "--setting=agent_interactive.new_isolation_dir_enabled=false",
                DIRECT,
            ),
            (*ENABLED,),
            (*ENABLED, "--agent-context=direct_call=false"),
        ]:
            result = await buck.audit_config(*args)
            assert "Buck advice" not in result.stderr
            await buck.clean()

    @buck_test()
    async def test_environment_isolation_is_guarded(self, buck: Buck) -> None:
        isolation = uuid.uuid4().hex
        buck.set_isolation_prefix("")
        try:
            await expect_failure(
                buck.audit_config(
                    *ENABLED, DIRECT, env={"BUCK_ISOLATION_DIR": isolation}
                ),
                stderr_regex=r"Buck advice \[new_isolation_dir\]",
            )
        finally:
            buck.set_isolation_prefix(isolation)
