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
from buck2.tests.e2e_util.helper.golden import golden


AGENT_MATERIALIZATION = "--setting=agent_materialization.enabled=true"
DIRECT = "--agent-context=direct_call=true"
MATERIALIZATION_NOTICE = "Final-output materialization was automatically disabled"
MATERIALIZATION_ENABLED_TAG = "agent_materialization.enabled"
MATERIALIZATION_DISABLED_TAG = "agent_materialization.disabled"


class BuildOutputTest(BuckTestCase):
    @buck_test()
    async def test_build_output(self, buck: Buck) -> None:
        show_output = await buck.build_without_report(
            "root//:foo",
            "--show-output",
        )
        show_full_output = await buck.build_without_report(
            "root//:foo",
            "--show-full-output",
        )
        show_simple_output = await buck.build_without_report(
            "root//:foo",
            "--show-simple-output",
        )
        show_json_output = await buck.build_without_report(
            "root//:foo",
            "--show-json-output",
        )

        output = "\n\n".join(
            [
                show_output.stdout,
                show_full_output.stdout,
                show_simple_output.stdout,
                show_json_output.stdout,
            ]
        )
        output = output.replace(str(buck.cwd), "/abs/project/root")
        output = output.replace("\\\\", "/")  # Windows path separators in json
        output = output.replace("\\", "/")  # Windows path separators not in json

        golden(
            output=output,
            rel_path="build_output.golden",
        )

    @buck_test()
    async def test_build_output_on_partial_success(self, buck: Buck) -> None:
        show_output = await expect_failure(
            buck.build_without_report(
                "root//:foo",
                "root//:fail",
                "--show-simple-output",
            )
        )
        assert len(show_output.stdout.splitlines()) == 1

        skipped_outputs = await expect_failure(
            buck.build_without_report(
                "root//:foo", "root//:fail", AGENT_MATERIALIZATION, DIRECT
            )
        )
        assert MATERIALIZATION_NOTICE not in skipped_outputs.stderr

    @buck_test(
        extra_buck_config={
            "buck2": {"materializations": "deferred", "defer_write_actions": "true"}
        },
        write_invocation_record=True,
    )
    async def test_agent_materialization_activation_and_overrides(
        self, buck: Buck
    ) -> None:
        cases = [
            ("direct", (AGENT_MATERIALIZATION, DIRECT), False),
            ("disabled", (DIRECT,), True),
            ("tool", (AGENT_MATERIALIZATION,), True),
            (
                "explicit_all",
                (AGENT_MATERIALIZATION, DIRECT, "--materializations=all"),
                True,
            ),
            (
                "explicit_none",
                (
                    AGENT_MATERIALIZATION,
                    DIRECT,
                    "--materializations=none",
                    "--show-output",
                ),
                False,
            ),
        ]
        # Only builds the setting decides record its state; the rest are outside its A/B test.
        expected_tags = {
            "direct": MATERIALIZATION_ENABLED_TAG,
            "disabled": MATERIALIZATION_DISABLED_TAG,
        }
        # Distinct outputs keep earlier materializations from masking a later skip.
        (buck.cwd / "TARGETS.fixture").write_text(
            "\n".join(f'trivial_build(name="{name}")' for name, _, _ in cases)
        )
        await buck.audit_config()
        daemon_id = json.loads((await buck.status()).stdout)["daemon_constraints"][
            "daemon_id"
        ]
        for name, args, materialized in cases:
            target = f"root//:{name}"
            result = await buck.build(
                target,
                *args,
                env={"CODING_AGENT_METADATA": "id=codex"},
            )
            output = result.get_build_report().output_for_target(target)
            assert output.exists() == materialized, name
            assert result.stderr.count(MATERIALIZATION_NOTICE) == (
                1 if name == "direct" else 0
            ), name
            assert MATERIALIZATION_NOTICE not in result.stdout
            tags = result.invocation_record()["tags"]
            expected_tag = expected_tags.get(name)
            assert {
                tag
                for tag in (MATERIALIZATION_ENABLED_TAG, MATERIALIZATION_DISABLED_TAG)
                if tag in tags
            } == ({expected_tag} if expected_tag else set()), name
            assert (
                json.loads((await buck.status()).stdout)["daemon_constraints"][
                    "daemon_id"
                ]
                == daemon_id
            ), name
            if name == "direct":
                for option in (
                    "--out <path>",
                    "--show-output",
                    "--show-full-output",
                ):
                    assert option in result.stderr
            if name in ("direct", "tool"):
                queried = await buck.targets(
                    target, "--show-output", AGENT_MATERIALIZATION
                )
                path = buck.cwd / queried.stdout.split()[1]
                assert path == output
                assert path.exists() == materialized, name
                if materialized:
                    assert path.read_text() == "abcd"

    @buck_test(write_invocation_record=True)
    async def test_agent_materialization_tag_is_recorded_for_failed_builds(
        self, buck: Buck
    ) -> None:
        # The notice only follows a successful build, but the tag is recorded before building.
        for args, expected_tag in [
            ((AGENT_MATERIALIZATION, DIRECT), MATERIALIZATION_ENABLED_TAG),
            ((DIRECT,), MATERIALIZATION_DISABLED_TAG),
        ]:
            result = await expect_failure(buck.build("root//:does_not_exist", *args))
            assert MATERIALIZATION_NOTICE not in result.stderr, args
            assert expected_tag in result.invocation_record()["tags"], args

    @buck_test(
        extra_buck_config={
            "buck2": {"materializations": "deferred", "defer_write_actions": "true"}
        }
    )
    async def test_agent_materialization_preserves_output_requests(
        self, buck: Buck
    ) -> None:
        cases = [
            ("--show-output",),
            ("--show-full-output",),
            ("--show-simple-output",),
            ("--show-full-simple-output",),
            ("--show-json-output",),
            ("--show-full-json-output",),
            ("--out=copied.txt",),
            ("--out=-",),
        ]
        (buck.cwd / "TARGETS.fixture").write_text(
            "\n".join(f'trivial_build(name="case_{i}")' for i in range(len(cases)))
            + '\ntrivial_build(name="custom_default")\n'
        )
        for i, args in enumerate(cases):
            target = f"root//:case_{i}"
            result = await buck.build(target, AGENT_MATERIALIZATION, DIRECT, *args)
            assert MATERIALIZATION_NOTICE not in result.stderr, args
            output = result.get_build_report().output_for_target(target)
            assert output.read_text() == "abcd", args
            if args == ("--out=copied.txt",):
                assert (buck.cwd / "copied.txt").read_text() == "abcd"
            elif args == ("--out=-",):
                assert "abcd" in result.stdout

        # Output requests preserve the configured default rather than forcing materialization.
        (buck.cwd / ".buckconfig.local").write_text(
            "[buck2]\nmaterializations = deferred_skip_final_artifacts\n"
        )
        target = "root//:custom_default"
        result = await buck.build(
            target, AGENT_MATERIALIZATION, DIRECT, "--show-output"
        )
        assert not result.get_build_report().output_for_target(target).exists()
