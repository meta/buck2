# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os
import subprocess
import tempfile
import unittest


class Buck2E2EBinSmokeTest(unittest.TestCase):
    """The `buck2_e2e-bin` test binary builds, starts, and reports `--version`."""

    def _run_version(
        self, extra_env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        binary = os.environ["BUCK2_E2E_BIN"]
        env = dict(os.environ)
        env.update(extra_env or {})
        return subprocess.run(
            [binary, "--version"],
            capture_output=True,
            text=True,
            timeout=180,
            env=env,
        )

    def test_version(self) -> None:
        result = self._run_version()
        self.assertEqual(result.returncode, 0, msg=f"stderr: {result.stderr}")

    def test_version_with_settings_override_set(self) -> None:
        # Every e2e run sets `BUCK2_SETTINGS_OVERRIDE`; startup must tolerate it.
        with tempfile.TemporaryDirectory() as tmp:
            override = os.path.join(tmp, "test.bucksettings.toml")
            with open(override, "w") as f:
                f.write('[cells]\nroot = "."\n')
            result = self._run_version({"BUCK2_SETTINGS_OVERRIDE": override})
        self.assertEqual(result.returncode, 0, msg=f"stderr: {result.stderr}")
