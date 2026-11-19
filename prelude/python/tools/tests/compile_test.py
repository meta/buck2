# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

COMPILE: Path = Path(__file__).resolve().parent.parent / "compile.py"


def _compile(tmp: Path, manifest: list[list[str]]) -> list[str]:
    manifest_path = tmp / "manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf8")
    bytecode_manifest = tmp / "bytecode.manifest"
    proc = subprocess.run(
        [
            sys.executable,
            "-B",
            str(COMPILE),
            "--output",
            str(tmp / "out"),
            "--bytecode-manifest",
            str(bytecode_manifest),
            str(manifest_path),
        ],
        capture_output=True,
        encoding="utf8",
        cwd=tmp,
    )
    if proc.returncode != 0:
        raise AssertionError(proc.stderr)
    return [dest for dest, _, _ in json.loads(bytecode_manifest.read_text())]


def _two_sources(tmp: Path) -> list[list[str]]:
    (tmp / "c.py").write_text("", encoding="utf8")
    (tmp / "b.c.py").write_text("", encoding="utf8")
    return [
        ["a/b/c.py", str(tmp / "c.py"), "//x:x"],
        ["a/b.c.py", str(tmp / "b.c.py"), "//x:x"],
    ]


class PycPathTest(unittest.TestCase):
    def test_a_dotted_file_name_keeps_its_own_bytecode_path(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            dests = _compile(tmp, _two_sources(tmp))
        self.assertEqual(dests[0], importlib.util.cache_from_source("a/b/c.py"))
        self.assertEqual(dests[1], importlib.util.cache_from_source("a/b.c.py"))
