# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import contextlib
import io
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import check_jdk


def make_jdk(root: Path, version: str) -> Path:
    (root / "bin").mkdir(parents=True)
    java = root / "bin" / "java"
    java.write_text("#!/bin/sh\n")
    java.chmod(0o755)
    (root / "release").write_text(f'IMPLEMENTOR="x"\nJAVA_VERSION="{version}"\n')
    return java


def run(*args: str) -> tuple[int, str]:
    err = io.StringIO()
    with (
        mock.patch.object(sys, "argv", ["check_jdk", *args]),
        contextlib.redirect_stderr(err),
    ):
        return check_jdk.main(), err.getvalue()


class CheckJdkTest(unittest.TestCase):
    def test_major_version(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            for version, major in [("17.0.12", 17), ("21", 21), ("1.8.0_402", 8)]:
                (Path(d) / "release").write_text(f'JAVA_VERSION="{version}"\n')
                self.assertEqual(check_jdk.jdk_major_version(d), major)
            os.remove(Path(d) / "release")
            self.assertIsNone(check_jdk.jdk_major_version(d))

    def test_new_enough_jdk_writes_the_stamp(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            java = make_jdk(Path(d) / "jdk", "21.0.5")
            out = Path(d) / "stamp"
            code, err = run(
                "--tool", str(java), "--java-version", "21", "--output", str(out)
            )
            self.assertEqual((code, err), (0, ""))
            self.assertTrue(out.exists())

    def test_old_jdk(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            java = make_jdk(Path(d) / "jdk", "17.0.12")
            out = Path(d) / "stamp"
            code, err = run(
                "--tool", str(java), "--java-version", "21", "--output", str(out)
            )
            self.assertEqual(code, 1)
            self.assertIn("is from JDK 17", err)
            self.assertIn("-c java.buck2_java_version=17", err)
            self.assertFalse(out.exists())

    def test_jdk_too_old_for_the_override(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            java = make_jdk(Path(d) / "jdk", "11.0.2")
            code, err = run(
                "--tool",
                str(java),
                "--java-version",
                "21",
                "--output",
                str(Path(d) / "stamp"),
            )
            self.assertEqual(code, 1)
            self.assertNotIn("buck2_java_version=11", err)

    def test_missing_tool_under_java_home(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            code, err = run(
                "--tool",
                f"{d}/bin/java",
                "--java-home",
                d,
                "--java-version",
                "21",
                "--output",
                f"{d}/stamp",
            )
            self.assertEqual(code, 1)
            self.assertIn(f"`[java] java_home` is `{d}`", err)

    def test_missing_tool_on_path(self) -> None:
        with (
            tempfile.TemporaryDirectory() as d,
            mock.patch.dict(os.environ, {"PATH": d}),
        ):
            code, err = run(
                "--tool", "javac", "--java-version", "21", "--output", f"{d}/stamp"
            )
            self.assertEqual(code, 1)
            self.assertIn("`javac` is not on `PATH`", err)
