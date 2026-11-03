# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import json
from typing import Any, Dict

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


async def _report(buck: Buck) -> Dict[str, Any]:
    result = await buck.audit("settings", "--json")
    return json.loads(result.stdout)


class SettingsRolloutsTest(BuckTestCase):
    # The unknown key below is reported as a soft error.
    @buck_test(allow_soft_errors=True)
    async def test_stanza_selection_and_per_key_tolerance(self, buck: Buck) -> None:
        versions = {s["name"]: s["version"] for s in (await _report(buck))["sections"]}
        version = versions["analysis"]
        assert version >= 1, "the test needs an older version to write a stanza for"

        # Only the stanza at or below the compiled version applies, and within
        # it a key this binary does not know is dropped on its own.
        rollouts = buck.get_settings_home_dir() / ".bucksettings.rollouts"
        rollouts.write_text(
            f"[analysis.{version - 1}]\n"
            "record_requested_anon_targets = false\n"
            "not_a_setting = true\n"
            f"[analysis.{version + 1}]\n"
            "record_requested_anon_targets = true\n"
        )

        report = await _report(buck)
        anon = next(
            setting
            for section in report["sections"]
            if section["name"] == "analysis"
            for setting in section["settings"]
            if setting["name"] == "record_requested_anon_targets"
        )
        assert anon["value"] is False
        assert anon["origin"]["kind"] == "rollout"

        rollout_layer = next(
            layer for layer in report["layers"] if layer["origin"]["kind"] == "rollout"
        )
        assert rollout_layer["applied"] is True
        assert [ignored["setting"] for ignored in rollout_layer["ignored"]] == [
            "analysis.not_a_setting"
        ]
