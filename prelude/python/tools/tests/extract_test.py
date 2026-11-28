# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import json
import os
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

EXTRACT: Path = Path(__file__).resolve().parent.parent / "extract.py"

DEMO = """\
import sys


def main():
    print("main", sys.argv[1:])


class _Program:
    def run(self):
        print("program.run", sys.argv[1:])


program = _Program()
"""


def _wheel(tmp: Path, console_scripts: str) -> Path:
    wheel = tmp / "demo-1.0-py3-none-any.whl"
    with zipfile.ZipFile(wheel, "w") as zf:
        zf.writestr("demo/__init__.py", DEMO)
        zf.writestr("demo-1.0.dist-info/METADATA", "Name: demo\nVersion: 1.0\n")
        zf.writestr(
            "demo-1.0.dist-info/entry_points.txt",
            "[console_scripts]\n" + console_scripts,
        )
    return wheel


def _extract(tmp: Path, wheel: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(EXTRACT),
            "--output",
            str(tmp / "out"),
            "--entry-points",
            str(tmp / "bin"),
            "--entry-points-manifest",
            str(tmp / "bin.json"),
            str(wheel),
        ],
        capture_output=True,
        encoding="utf8",
    )


def _run_script(tmp: Path, name: str) -> str:
    proc = subprocess.run(
        [sys.executable, str(tmp / "bin" / name), "a"],
        capture_output=True,
        encoding="utf8",
        env={**os.environ, "PYTHONPATH": str(tmp / "out")},
    )
    if proc.returncode != 0:
        raise AssertionError(proc.stderr)
    return proc.stdout


class ConsoleScriptsTest(unittest.TestCase):
    def test_object_references(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            wheel = _wheel(
                tmp,
                "plain = demo:main\n"
                "dotted = demo:program.run\n"
                "extras = demo:main [cli]\n"
                "MixedCase = demo:main\n",
            )
            proc = _extract(tmp, wheel)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            names = [name for name, _, _ in json.loads((tmp / "bin.json").read_text())]
            self.assertEqual(names, ["plain", "dotted", "extras", "MixedCase"])
            self.assertEqual(_run_script(tmp, "plain"), "main ['a']\n")
            self.assertEqual(_run_script(tmp, "dotted"), "program.run ['a']\n")
            self.assertEqual(_run_script(tmp, "extras"), "main ['a']\n")
            self.assertEqual(_run_script(tmp, "MixedCase"), "main ['a']\n")

    def test_a_reference_without_a_callable_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            proc = _extract(tmp, _wheel(tmp, "raw = demo.__main__\n"))
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("Invalid console script entry point raw", proc.stderr)
