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


def _entries(report: Dict[str, Any]) -> Dict[str, Dict[str, Any]]:
    return {
        f"{section['name']}.{setting['name']}": setting
        for section in report["sections"]
        for setting in section["settings"]
    }


class AuditSettingsTest(BuckTestCase):
    @buck_test()
    async def test_report_names_values_and_origins(self, buck: Buck) -> None:
        (buck.cwd / ".bucksettings.toml").write_text(
            "[analysis]\nrecord_requested_anon_targets = false\n"
        )
        home_local = buck.get_settings_home_dir() / ".bucksettings.local.toml"
        home_local.write_text('[log_download]\nlog_url = "https://home/"\n')
        with open(buck.cwd / ".buckconfig", "a") as buckconfig:
            buckconfig.write("[buck2]\ndaemon_idle_timeout_s = 100\n")

        result = await buck.audit(
            "settings",
            "--json",
            "--setting",
            "agent_materialization.enabled=true",
        )
        report = json.loads(result.stdout)
        assert report["hash"].startswith("buck2settings-")

        layers = {
            (layer["origin"]["kind"], layer["applied"]) for layer in report["layers"]
        }
        assert ("repository", True) in layers
        assert ("local", True) in layers
        assert ("command_line", True) in layers

        entries = _entries(report)
        anon = entries["analysis.record_requested_anon_targets"]
        assert anon["value"] is False
        assert anon["origin"]["kind"] == "repository"
        assert anon["internal_default"] is True

        log_url = entries["log_download.log_url"]
        assert log_url["value"] == "https://home/"
        assert log_url["origin"]["kind"] == "local"
        assert log_url["origin"]["path"] == str(home_local)

        enabled = entries["agent_materialization.enabled"]
        assert enabled["value"] is True
        assert enabled["origin"]["kind"] == "command_line"

        defer = entries["hydration.defer_field_reads"]
        assert defer["value"] is True
        assert defer["origin"] is None

        # A declared fallback is read from the root buckconfig when no layer
        # sets the key.
        idle = entries["daemon.idle_timeout_s"]
        assert idle["value"] == 100
        assert idle["set"] is None
        assert idle["origin"] == {
            "kind": "buckconfig",
            "path": "buck2.daemon_idle_timeout_s",
        }
        assert idle["buckconfig_fallback"]["key"] == "buck2.daemon_idle_timeout_s"

        scopes = {section["name"]: section["scope"] for section in report["sections"]}
        assert scopes["hydration"] == "daemon"
        assert scopes["agent_materialization"] == "invocation"

    @buck_test()
    async def test_text_report(self, buck: Buck) -> None:
        (buck.cwd / ".bucksettings.toml").write_text(
            "[analysis]\nrecord_requested_anon_targets = false\n"
        )
        result = await buck.audit("settings", "--verbose")
        assert "Buck settings buck2settings-" in result.stdout
        assert "Layers, lowest priority first:" in result.stdout
        assert "[analysis]  daemon scope, version" in result.stdout
        assert "record_requested_anon_targets = false" in result.stdout
        assert "(repository)" in result.stdout
        assert "defer_field_reads = true" in result.stdout
        assert "(default)" in result.stdout
        assert "overridable in:" in result.stdout
