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
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


ENABLED = (
    "--setting=agent_interactive.enabled=true",
    "--setting=agent_interactive.clean_enabled=true",
)
DIRECT = "--agent-context=direct_call=true"


class AgentCleanTest(BuckTestCase):
    @buck_test(write_invocation_record=True)
    async def test_block_preserves_daemon_and_outputs(self, buck: Buck) -> None:
        buck.set_isolation_prefix(uuid.uuid4().hex)
        assert buck.isolation_prefix
        await buck.audit_config(*ENABLED)
        before = json.loads((await buck.status()).stdout)["process_info"]["pid"]
        output = buck.cwd / "buck-out" / buck.isolation_prefix / "output.txt"
        output.write_text("preserve this output")

        for args in [(), ("--background",)]:
            result = await expect_failure(
                buck.clean(*args, *ENABLED, DIRECT),
                stderr_regex=r"Buck advice \[clean\]",
            )
            assert output.read_text() == "preserve this output"
            assert (
                json.loads((await buck.status()).stdout)["process_info"]["pid"]
                == before
            )
            record = result.invocation_record()
            assert "agent_interactive.clean.block" in record["tags"]
            assert "agent_interactive.clean.enabled" in record["tags"]
            assert "AGENT_ADVICE_BLOCKED" in record.single_error()["tags"]

    @buck_test(write_invocation_record=True)
    async def test_clean_requires_its_own_ack_and_nonempty_reason(
        self, buck: Buck
    ) -> None:
        buck.set_isolation_prefix(uuid.uuid4().hex)
        assert buck.isolation_prefix
        await buck.audit_config()
        output = buck.cwd / "buck-out" / buck.isolation_prefix / "output.txt"
        output.write_text("remove only after acknowledgement")
        for context in [
            "advice_ack=killall,reason=Reset state",
            "advice_ack=new_isolation_dir,reason=Reset state",
            "advice_ack=clean",
            "advice_ack=clean,reason=   ",
            "reason=Reset state",
        ]:
            await expect_failure(
                buck.clean(*ENABLED, DIRECT, f"--agent-context={context}"),
                stderr_regex=r"Buck advice \[clean\]",
            )
            assert output.exists()

        result = await buck.clean(
            *ENABLED,
            DIRECT,
            "--agent-context=advice_ack=clean,reason=Reset state for this test",
        )
        assert not output.exists()
        assert "no buckd running" in (await buck.status()).stderr
        assert "agent_interactive.clean.ack" in result.invocation_record()["tags"]

    @buck_test(write_invocation_record=True)
    async def test_disabled_direct_clean_records_its_state(self, buck: Buck) -> None:
        buck.set_isolation_prefix(uuid.uuid4().hex)
        assert buck.isolation_prefix
        await buck.audit_config()
        output = buck.cwd / "buck-out" / buck.isolation_prefix / "output.txt"
        output.write_text("remove this output")
        result = await buck.clean(DIRECT)
        assert "Buck advice" not in result.stderr
        assert not output.exists()
        assert "agent_interactive.clean.disabled" in result.invocation_record()["tags"]

        # A dry run, a stale cleanup, or a call that is not direct is not recorded.
        await buck.audit_config()
        for args in [("--dry-run", DIRECT), ("--stale=1d", DIRECT), ()]:
            result = await buck.clean(*args)
            assert (
                "agent_interactive.clean.disabled"
                not in result.invocation_record()["tags"]
            ), args

    @buck_test()
    async def test_dry_run_and_stale_do_not_require_full_clean_ack(
        self, buck: Buck
    ) -> None:
        buck.set_isolation_prefix(uuid.uuid4().hex)
        assert buck.isolation_prefix
        await buck.audit_config(*ENABLED)
        before = json.loads((await buck.status()).stdout)["process_info"]["pid"]
        output = buck.cwd / "buck-out" / buck.isolation_prefix / "output.txt"
        output.write_text("preserve this output")
        for args in [(), ("--background",), ("--stale=1d",)]:
            result = await buck.clean("--dry-run", *args, *ENABLED, DIRECT)
            assert "Buck advice" not in result.stderr
            assert output.read_text() == "preserve this output"
            assert (
                json.loads((await buck.status()).stdout)["process_info"]["pid"]
                == before
            )

    @buck_test()
    async def test_disabled_and_non_direct_clean_keep_existing_behavior(
        self, buck: Buck
    ) -> None:
        buck.set_isolation_prefix(uuid.uuid4().hex)
        assert buck.isolation_prefix
        for args in [
            (DIRECT,),
            (*ENABLED, "--setting=agent_interactive.clean_enabled=false", DIRECT),
            (*ENABLED,),
            (*ENABLED, "--agent-context=direct_call=false"),
        ]:
            await buck.audit_config()
            output = buck.cwd / "buck-out" / buck.isolation_prefix / "output.txt"
            output.write_text("remove this output")
            result = await buck.clean(*args)
            assert "Buck advice" not in result.stderr
            assert not output.exists()
            assert "no buckd running" in (await buck.status()).stderr

    @buck_test()
    async def test_clean_guard_works_with_invalid_build_config(
        self, buck: Buck
    ) -> None:
        (buck.cwd / ".buckconfig").write_text("invalid buckconfig")
        await expect_failure(
            buck.clean(*ENABLED, DIRECT),
            stderr_regex=r"Buck advice \[clean\]",
        )
