#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os
import pathlib
import shutil
import sys
import tempfile
import unittest
from unittest import mock

from buildscript_run import main


class BuildscriptRunTest(unittest.TestCase):
    """`cargo_buildscript` runs a crate's build script through `buildscript_run.py` and feeds the
    `cargo:` directives it prints to rustc as flags."""

    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)

    def _executable(self, name: str, script: str) -> pathlib.Path:
        path = self.tmp / name
        path.write_text("#!/bin/sh\n" + script)
        path.chmod(0o755)
        return path

    def rustc_flags_for(self, output_lines: list) -> list:
        """The rustc flags produced for a build script that prints `output_lines`."""
        buildscript = self._executable(
            "build-script-build",
            "".join("printf '%s\\n' '" + line + "'\n" for line in output_lines),
        )
        rustc = self._executable("rustc", "echo rustc 1.0.0\n")
        rustc_cfg = self.tmp / "rustc_cfg"
        rustc_cfg.write_text("unix\n")
        manifest_dir = self.tmp / "manifest"
        manifest_dir.mkdir()
        outfile = self.tmp / "rustc_flags"
        argv = [
            "buildscript_run.py",
            f"--buildscript={buildscript}",
            f"--rustc-cfg={rustc_cfg}",
            f"--manifest-dir={manifest_dir}",
            f"--create-cwd={self.tmp / 'cwd'}",
            f"--outfile={outfile}",
        ]
        env = {
            **os.environ,
            "OUT_DIR": str(self.tmp / "out"),
            "RUSTC": str(rustc),
            "TARGET": "x86_64-unknown-linux-gnu",
            "HOST": "x86_64-unknown-linux-gnu",
        }
        with mock.patch.object(sys, "argv", argv), mock.patch.dict(os.environ, env):
            main()
        return outfile.read_text().splitlines()

    def test_a_directive_is_read_from_the_trimmed_line(self) -> None:
        self.assertEqual(
            self.rustc_flags_for(
                [
                    "cargo:rustc-env=TRAIL=v  ",
                    "cargo:rustc-cfg=feature_x ",
                    "  cargo:rustc-cfg=y",
                ]
            ),
            ["--env-set=TRAIL=v", "--cfg=feature_x", "--cfg=y"],
        )
